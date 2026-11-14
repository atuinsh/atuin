use std::fmt::Display;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rusty_paseto::core::{
    ImplicitAssertion, Key as PasetoKey, Local, Paseto, PasetoNonce, PasetoSymmetricKey,
    Payload as PasetoPayload, V4,
};
use serde::{Deserialize, Serialize};

use super::bytes::EncryptedBytes;
use super::envelope::{Cek, KidMatched};
use super::error::{
    AuthenticationError, LegacyCause, MalformedError, OpenError, SealError, ShapeError,
};
use super::{EncryptedData, Key, KeyId, Layer};

/// The one `rusty_paseto` type outside this file, inside [`LegacyCause`].
pub(super) type PasetoError = rusty_paseto::core::PasetoError;

type PaserkV4PieWrappedKey = rusty_paserk::PieWrappedKey<rusty_paserk::V4, rusty_paserk::Local>;

/// The legacy `cek` JSON, parsed as every released atuin parses it.
#[derive(Deserialize)]
struct CekJson {
    wpk: PaserkV4PieWrappedKey,
    kid: String,
}

/// The plaintext of a legacy token: PASETO only takes strings.
#[derive(Serialize, Deserialize)]
struct EncryptedJson {
    data: String,
}

/// A PASERK `k4.local-wrap.pie` content-key wrap and the id of its wrapping key.
pub(super) struct PieWrap {
    /// Consumed by the unwrap: `PieWrappedKey` is not `Clone`.
    wpk: PaserkV4PieWrappedKey,
    kid: KeyId,
}

impl PieWrap {
    /// Parse leniently, like every released atuin: any key order, whitespace, unknown keys.
    pub(super) fn parse(cek: &str) -> Result<Self, ShapeError> {
        let CekJson { wpk, kid } = serde_json::from_str(cek).map_err(ShapeError::LegacyCekJson)?;
        Ok(Self {
            wpk,
            kid: kid.parse().map_err(ShapeError::LegacyKeyId)?,
        })
    }

    pub(super) const fn key_id(&self) -> KeyId {
        self.kid
    }

    /// Unwrap the content key with `master`, whose kid `proof` says this wrap names.
    pub(super) fn unwrap_cek(self, master: &Key, _proof: KidMatched) -> Result<Cek, OpenError> {
        let cek = self
            .wpk
            .unwrap_key(&master.to_paserk())
            .map_err(|err| LegacyCause::classify(err, Layer::Wrap))?;
        Ok(Cek::from(cek.to_bytes()))
    }

    /// Wrap `cek` under `master` as the canonical JSON.
    pub(super) fn seal(cek: &Cek, master: &Key) -> String {
        let cek =
            rusty_paserk::Key::<rusty_paserk::V4, rusty_paserk::Local>::from_bytes(*cek.as_bytes());
        Self::canonical_json(cek.wrap_pie(&master.to_paserk()), master.key_id())
    }

    /// `{"wpk":"<wpk>","kid":"<kid>"}`: the layout legacy readers parse and [`LegacyPacked`] rebuilds byte for byte.
    pub(super) fn canonical_json(wpk: impl Display, kid: impl Display) -> String {
        // PASERK strings are `[A-Za-z0-9_.-]`, so neither needs JSON escaping.
        format!(r#"{{"wpk":"{wpk}","kid":"{kid}"}}"#)
    }
}

/// A legacy `v4.local.` token over `{"data": base64url(plaintext)}`.
pub(super) struct LegacyToken<'a>(pub(super) &'a str);

impl LegacyToken<'_> {
    pub(super) const PREFIX: &'static str = "v4.local.";
    /// rusty_paseto 0.8 slices the decoded body at 32 and len - 32 unchecked (v4_local.rs:36,43); 86 is the shortest body that decodes to 64 bytes.
    const MIN_BODY_CHARS: usize = 86;

    pub(super) fn open(&self, cek: &Cek, aad: &str) -> Result<Vec<u8>, OpenError> {
        // `c` unpadded characters decode to `floor(6c / 8)` bytes and `c % 4 == 1` never decodes,
        // so this rejects exactly the bodies that would decode short, without decoding anything.
        let chars = self.0.split('.').nth(2).map_or(0, str::len);
        if chars < Self::MIN_BODY_CHARS {
            return Err(MalformedError::LegacyShortBody { chars }.into());
        }

        let payload = Paseto::<V4, Local>::try_decrypt(
            self.0,
            &cek.to_paseto(),
            None,
            Some(ImplicitAssertion::from(aad)),
        )
        .map_err(|err| LegacyCause::classify(err, Layer::Payload))?;
        let EncryptedJson { data } =
            serde_json::from_str(&payload).map_err(MalformedError::LegacyPayloadJson)?;
        URL_SAFE_NO_PAD.decode(data).map_err(|err| MalformedError::LegacyPayloadBase64(err).into())
    }

    /// Seal `plaintext` under `cek` as a v4.local token over `{"data": base64url(plaintext)}`, bound to `aad`.
    pub(super) fn seal(cek: &Cek, aad: &str, plaintext: &[u8]) -> Result<String, SealError> {
        let payload = serde_json::to_string(&EncryptedJson {
            data: URL_SAFE_NO_PAD.encode(plaintext),
        })
        .map_err(SealError::LegacyJson)?;
        let mut nonce = [0; 32];
        getrandom::fill(&mut nonce)?;
        let nonce = PasetoKey::<32>::from(nonce);

        Paseto::<V4, Local>::builder()
            .set_payload(PasetoPayload::from(payload.as_str()))
            .set_implicit_assertion(ImplicitAssertion::from(aad))
            .try_encrypt(&cek.to_paseto(), &PasetoNonce::<V4, Local>::from(&nonce))
            .map_err(|err| SealError::Legacy(LegacyCause(err)))
    }
}

/// A canonical legacy value with its base64 and JSON stripped.
///
/// Packing is the trust boundary: it refuses anything not in the canonical wire form rather than
/// guess at it, so every packed value unpacks to exactly the strings it came from.
pub(super) struct LegacyPacked {
    data: Vec<u8>,
    cek: [u8; EncryptedBytes::LEGACY_CEK_LEN],
}

impl LegacyPacked {
    const WPK_PREFIX: &'static str = "k4.local-wrap.pie.";
    const WPK_LEN: usize = 96;

    /// Pack `value` only if unpacking reproduces both strings byte for byte.
    pub(super) fn pack(value: &EncryptedData) -> Option<Self> {
        // `URL_SAFE_NO_PAD` rejects padding and non-zero trailing bits, so a successful decode
        // re-encodes to the same string.
        let data = URL_SAFE_NO_PAD.decode(value.raw.strip_prefix(LegacyToken::PREFIX)?).ok()?;

        let json: serde_json::Value = serde_json::from_str(&value.cek).ok()?;
        let wpk = json.get("wpk")?.as_str()?.strip_prefix(Self::WPK_PREFIX)?;
        let wpk = URL_SAFE_NO_PAD.decode(wpk).ok()?;
        let kid: KeyId = json.get("kid")?.as_str()?.parse().ok()?;
        let cek = [wpk.as_slice(), kid.as_bytes()].concat().try_into().ok()?;

        // The JSON must round-trip too: field order and whitespace are not ours to normalise.
        (Self::unpack_cek(&cek) == value.cek).then_some(Self { data, cek })
    }

    pub(super) fn unpack(
        data: Vec<u8>,
        cek: &[u8; EncryptedBytes::LEGACY_CEK_LEN],
    ) -> EncryptedData {
        EncryptedData {
            raw: format!("{}{}", LegacyToken::PREFIX, URL_SAFE_NO_PAD.encode(data)),
            cek: Self::unpack_cek(cek),
        }
    }

    fn unpack_cek(cek: &[u8; EncryptedBytes::LEGACY_CEK_LEN]) -> String {
        let (wpk, kid) = cek.split_at(Self::WPK_LEN);
        PieWrap::canonical_json(
            format_args!("{}{}", Self::WPK_PREFIX, URL_SAFE_NO_PAD.encode(wpk)),
            KeyId::from_bytes(kid.try_into().expect("a packed cek ends in a key id")),
        )
    }

    pub(super) fn into_bytes(self) -> EncryptedBytes {
        EncryptedBytes {
            data: self.data,
            cek: self.cek.to_vec(),
        }
    }
}

impl EncryptedData {
    /// Seal as a v4.local token over `{"data": b64}` with a PIE-wrapped content key.
    pub(super) fn seal_legacy(key: &Key, aad: &str, plaintext: &[u8]) -> Result<Self, SealError> {
        let cek = Cek::random()?;
        Ok(Self {
            raw: LegacyToken::seal(&cek, aad, plaintext)?,
            cek: PieWrap::seal(&cek, key),
        })
    }
}

impl Key {
    pub(super) fn to_paserk(&self) -> rusty_paserk::Key<rusty_paserk::V4, rusty_paserk::Local> {
        rusty_paserk::Key::from_bytes(*self.as_bytes())
    }
}

impl Cek {
    pub(super) fn to_paseto(&self) -> PasetoSymmetricKey<V4, Local> {
        PasetoSymmetricKey::from(PasetoKey::<32>::from(*self.as_bytes()))
    }
}

impl LegacyCause {
    /// Classify a rusty error at `layer`: a failed tag check is authentication, anything else malformed.
    pub(super) fn classify(err: PasetoError, layer: Layer) -> OpenError {
        // The only two tag-check failures: `v4_local.rs:58-60` (token) and `wrap.rs:233` (PIE).
        if matches!(err, PasetoError::Cryption | PasetoError::InvalidSignature) {
            AuthenticationError::Legacy {
                layer,
                cause: Self(err),
            }
            .into()
        } else {
            MalformedError::Legacy {
                layer,
                cause: Self(err),
            }
            .into()
        }
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use rstest::rstest;

    use super::*;
    use crate::encryption::error::ShapeError;
    use crate::encryption::tests::{GOLDEN_CEK, GOLDEN_RAW, KAT_AAD, key};
    use crate::encryption::{EnvelopeFormat, paseto_v4};

    /// Sealed by `paseto_v4::encrypt_sync` over the empty payload.
    const EMPTY_RAW: &str = "v4.local.888D_XWhzV4uatc552GhjOobrGPuDnu3Xl_rNfi-Jx7GKvXlj7BiWJIs8Q0vuBIz9IXNXt2b4deT5_ooBtUB8seq6Nh3cLDRort-";
    const EMPTY_CEK: &str = r#"{"wpk":"k4.local-wrap.pie.I-gW41sipxSjtAPRt8zbo2zEwuaiq3J6N9YpR-Nb8dt6OD5DDOGaB4sPdZT1Sc5Y7N7s5_zgH7nkywkd9kjkzvYi0jXyScg-3V3UuxXXC-NDKvWNQeoVc9k1MHwsYLXM","kid":"k4.lid.2LzCmxDtbwu2tK5T1X1VLEth8umaI9vbKgTDkkt7ARR0"}"#;

    /// The PASERK `k4.local-wrap.pie` vectors' wrapping key.
    const PIE_WRAPPING_KEY: [u8; 32] = [
        0x70, 0x71, 0x72, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7a, 0x7b, 0x7c, 0x7d, 0x7e,
        0x7f, 0x80, 0x81, 0x82, 0x83, 0x84, 0x85, 0x86, 0x87, 0x88, 0x89, 0x8a, 0x8b, 0x8c, 0x8d,
        0x8e, 0x8f,
    ];

    impl EncryptedData {
        fn legacy(raw: &str, cek: &str) -> Self {
            Self {
                raw: raw.to_owned(),
                cek: cek.to_owned(),
            }
        }
    }

    #[rstest]
    #[case::golden_record(GOLDEN_RAW, GOLDEN_CEK, &[1, 2, 3, 4, 5])]
    #[case::empty(EMPTY_RAW, EMPTY_CEK, &[])]
    fn opens_frozen_values(
        key: Key,
        #[case] raw: &str,
        #[case] cek: &str,
        #[case] plaintext: &[u8],
    ) {
        let value = EncryptedData::legacy(raw, cek);

        assert_eq!(value.open(&key, KAT_AAD).unwrap(), plaintext);
        assert_eq!(EncryptedData::from(value.to_bytes().unwrap()), value);
    }

    #[rstest]
    #[case::empty(0)]
    #[case::one(1)]
    #[case::half(42)]
    #[case::one_byte_short(84)]
    #[case::undecodable(85)]
    fn a_short_token_body_is_malformed(key: Key, #[case] chars: usize) {
        let value = EncryptedData::legacy(&format!("v4.local.{}", "A".repeat(chars)), GOLDEN_CEK);

        assert!(matches!(
            value.open(&key, KAT_AAD),
            Err(OpenError::Malformed(MalformedError::LegacyShortBody { chars: found })) if found == chars
        ));
    }

    #[rstest]
    fn the_shortest_body_reaches_the_tag_check(key: Key) {
        let value = EncryptedData::legacy(&format!("v4.local.{}", "A".repeat(86)), GOLDEN_CEK);

        assert!(matches!(
            value.open(&key, KAT_AAD),
            Err(OpenError::Authentication(AuthenticationError::Legacy {
                layer: Layer::Payload,
                ..
            }))
        ));
    }

    #[rstest]
    fn never_panics_on_an_arbitrary_token(key: Key) {
        proptest!(|(body in "[ -~]{0,200}")| {
            let value = EncryptedData::legacy(&format!("v4.local.{body}"), GOLDEN_CEK);
            prop_assert!(value.open(&key, KAT_AAD).is_err(), "{value:?}");
        });
    }

    #[rstest]
    fn a_tampered_token_fails_authentication(key: Key) {
        let mut raw = GOLDEN_RAW.to_owned();
        raw.replace_range(
            40..41,
            if &raw[40..41] == "A" {
                "B"
            } else {
                "A"
            },
        );

        assert!(matches!(
            EncryptedData::legacy(&raw, GOLDEN_CEK).open(&key, KAT_AAD),
            Err(OpenError::Authentication(AuthenticationError::Legacy {
                layer: Layer::Payload,
                ..
            }))
        ));
    }

    #[rstest]
    fn a_tampered_wrap_fails_authentication(key: Key) {
        let cek = GOLDEN_CEK.replacen("8xXPgr", "8xXPgs", 1);

        assert!(matches!(
            EncryptedData::legacy(GOLDEN_RAW, &cek).open(&key, KAT_AAD),
            Err(OpenError::Authentication(AuthenticationError::Legacy {
                layer: Layer::Wrap,
                ..
            }))
        ));
    }

    #[rstest]
    fn seals_the_canonical_layout(key: Key) {
        let sealed = EncryptedData::seal_as(EnvelopeFormat::Legacy, &key, KAT_AAD, b"ls").unwrap();

        assert!(sealed.raw.starts_with(LegacyToken::PREFIX), "{}", sealed.raw);
        let wrap = PieWrap::parse(&sealed.cek).unwrap();
        assert_eq!(sealed.cek, PieWrap::canonical_json(wrap.wpk, key.key_id()));
        assert!(LegacyPacked::pack(&sealed).is_some());
    }

    #[rstest]
    #[case::zero_key("k4.local-wrap.pie.y-PC8Zh6P1DoOBUdhRr7W8GWSgHtRKvE8PWWYA-qXy3fxJDmaRsxcZVQzuvXHZuBg5MqCgh_y5K0WbukJCrDX73Wdf631VBnE1DNHafbjnGNzFNWP59ba9ifsOAgE7Bw", [0x00; 32])]
    #[case::ones_key("k4.local-wrap.pie.cy-Mu6zSfhu6q0_XdAM9p1zre_joUWjreSjHgisVNh-oHaNarN4_c7xuSyaHwqEDxF7lTbfNplBGU7wTeUyt__hZyj1J38NdNxVwuXamJY2QhRE-kWYA9_16xTsGwCQX", [0xff; 32])]
    fn unwraps_the_paserk_pie_vectors(#[case] wpk: &str, #[case] unwrapped: [u8; 32]) {
        let wrapping = Key::from(PIE_WRAPPING_KEY);
        let value = EncryptedData {
            raw: LegacyToken::seal(&Cek::from(unwrapped), "", b"vector").unwrap(),
            cek: PieWrap::canonical_json(wpk, wrapping.key_id()),
        };

        assert_eq!(value.open(&wrapping, "").unwrap(), b"vector");
    }

    #[rstest]
    #[case::bad_tag("k4.local-wrap.pie.cy-Mu6zSfhu6q0_XdAM9p1zre_joUWjreSjHgisVNh-oHaNarN4_c7xuSyaHwqEDxF7lTbfNplBGU7wTeUyt__hZyj1J38NdNxVwuXamJY3QhRE-kWYA9_16xUtGwCQY", |e: &OpenError| matches!(e, OpenError::Authentication(AuthenticationError::Legacy { layer: Layer::Wrap, .. })))]
    #[case::wrong_version("k3.local-wrap.pie.bkbHfW4bBJQ8jcPfLOYxUrg4SkKRHbsywYZwRvxUGFt1je2idZxlFr8sbkB6jTZ6hnrVlI25G2hqZtfdQyFIUcrRAiBrCWNPP1b3afdD9_YxsAXoKEA3X4AZhReuvHCzuPqXNCtrvJtpupGZn-PLFQ", |e: &OpenError| matches!(e, OpenError::Malformed(MalformedError::Shape(ShapeError::LegacyCekJson(_)))))]
    fn rejects_the_failing_paserk_pie_vectors(
        #[case] wpk: &str,
        #[case] expected: fn(&OpenError) -> bool,
    ) {
        let wrapping = Key::from(PIE_WRAPPING_KEY);
        let value = EncryptedData {
            raw: LegacyToken::seal(&Cek::from([0xff; 32]), "", b"vector").unwrap(),
            cek: PieWrap::canonical_json(wpk, wrapping.key_id()),
        };

        let err = value.open(&wrapping, "").unwrap_err();
        assert!(expected(&err), "{err:?}");
    }

    #[rstest]
    #[case::empty_footer(format!("{GOLDEN_RAW}."), GOLDEN_CEK.to_owned())]
    #[case::reordered_cek(GOLDEN_RAW.to_owned(), {
        let wrap = PieWrap::parse(GOLDEN_CEK).unwrap();
        format!(r#"{{ "kid" : "{}", "wpk" : "{}", "extra": 1 }}"#, wrap.kid, wrap.wpk)
    })]
    fn still_opens_what_released_atuin_accepts(key: Key, #[case] raw: String, #[case] cek: String) {
        let value = EncryptedData { raw, cek };

        assert_eq!(value.open(&key, KAT_AAD).unwrap(), [1, 2, 3, 4, 5]);
        assert_eq!(value.to_bytes(), None);
    }

    #[rstest]
    #[case::small(16)]
    #[case::large(70_000)]
    fn agrees_with_the_paseto_v4_module(key: Key, #[case] len: usize) {
        let old_key = paseto_v4::Key::from(*key.as_bytes());
        let plaintext: Vec<u8> = (0..len).map(|i| i.to_le_bytes()[0]).collect();

        let new =
            EncryptedData::seal_as(EnvelopeFormat::Legacy, &key, KAT_AAD, &plaintext).unwrap();
        let as_old = paseto_v4::EncryptedData {
            raw: new.raw,
            cek: new.cek,
        };
        let assertion = Some(paseto_v4::ImplicitAssertion::from(KAT_AAD));
        assert_eq!(paseto_v4::decrypt_sync(&as_old, assertion, &old_key).unwrap(), plaintext);

        let old = paseto_v4::encrypt_sync(&plaintext, assertion, &old_key).unwrap();
        let as_new = EncryptedData::legacy(&old.raw, &old.cek);
        assert_eq!(as_new.open(&key, KAT_AAD).unwrap(), plaintext);

        let other = Key::generate();
        let rewrapped = as_new.rewrap(&key, &other).unwrap();
        let rewrapped = paseto_v4::EncryptedData {
            raw: rewrapped.raw,
            cek: rewrapped.cek,
        };
        let other = paseto_v4::Key::from(*other.as_bytes());
        assert_eq!(paseto_v4::decrypt_sync(&rewrapped, assertion, &other).unwrap(), plaintext);
    }

    #[rstest]
    fn agrees_with_the_paseto_v4_module_on_any_input(key: Key) {
        let old_key = paseto_v4::Key::from(*key.as_bytes());
        proptest!(ProptestConfig::with_cases(64), |(aad in "[ -~]{0,64}", plaintext in prop::collection::vec(any::<u8>(), 0..1024))| {
            let assertion = Some(paseto_v4::ImplicitAssertion::from(aad.as_str()));

            let new = EncryptedData::seal_as(EnvelopeFormat::Legacy, &key, &aad, &plaintext).unwrap();
            let as_old = paseto_v4::EncryptedData { raw: new.raw, cek: new.cek };
            prop_assert_eq!(&paseto_v4::decrypt_sync(&as_old, assertion, &old_key).unwrap(), &plaintext);

            let old = paseto_v4::encrypt_sync(&plaintext, assertion, &old_key).unwrap();
            prop_assert_eq!(EncryptedData::legacy(&old.raw, &old.cek).open(&key, &aad).unwrap(), plaintext);
        });
    }
}
