use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use super::avelope::{AvelopePayload, Draws, SymmetricWrap};
use super::error::{
    OpenError, ParseError, RewrapError, SealError, ShapeError, UnsupportedFormat, WrongKey,
};
use super::legacy::{LegacyToken, PieWrap};
use super::{EnvelopeFormat, Key, KeyId, Layer};

/// A sealed payload and its wrapped content key, as the two strings the server stores.
///
/// **Do not** rename the serde fields: clients and servers of every version exchange them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EncryptedData {
    /// The sealed payload.
    #[serde(rename = "data", alias = "raw")]
    pub raw: String,
    /// The wrapped content key.
    #[serde(rename = "content_encryption_key", alias = "cek")]
    pub cek: String,
}

impl EncryptedData {
    /// Seal `plaintext` under a fresh content key wrapped by `key`, in [`EnvelopeFormat::WRITE`].
    pub fn seal(key: &Key, aad: &str, plaintext: &[u8]) -> Result<Self, SealError> {
        Self::seal_as(EnvelopeFormat::WRITE, key, aad, plaintext)
    }

    /// Seal in `format`; production code calls [`Self::seal`].
    pub fn seal_as(
        format: EnvelopeFormat,
        key: &Key,
        aad: &str,
        plaintext: &[u8],
    ) -> Result<Self, SealError> {
        match format {
            EnvelopeFormat::Legacy => Self::seal_legacy(key, aad, plaintext),
            EnvelopeFormat::Avelope => Self::seal_avelope(key, aad, plaintext, Draws::random()?),
        }
    }

    /// Open a value sealed with `aad`, in any format this build reads.
    pub fn open(&self, key: &Key, aad: &str) -> Result<Vec<u8>, OpenError> {
        let wrap = Wrap::parse(&self.cek)?;
        let matched = wrap.matching(key)?;
        matched.open(Payload::parse(&self.raw)?, aad)
    }

    /// Rewrap the content key from `old` to `new` in its own wrap format; `raw` is untouched.
    pub fn rewrap(&self, old: &Key, new: &Key) -> Result<Self, RewrapError> {
        let wrap = Wrap::parse(&self.cek).map_err(OpenError::from)?;
        let cek = wrap.matching(old).map_err(OpenError::from)?.rewrap(new)?;
        Ok(Self {
            raw: self.raw.clone(),
            cek,
        })
    }

    /// The id of the key the content key is wrapped under, read without decrypting anything.
    pub fn key_id(&self) -> Result<KeyId, ParseError> {
        Wrap::parse(&self.cek).map(|wrap| wrap.key_id())
    }

    /// Whether the payload is in the avelope container, whatever its version.
    #[must_use]
    pub fn is_avelope(&self) -> bool {
        self.raw.starts_with(Self::AVELOPE_PREFIX)
    }
}

/// A content key, wiped on drop; it never leaves `encryption`.
pub(super) struct Cek(Zeroizing<[u8; 32]>);

impl Cek {
    pub(super) fn random() -> Result<Self, getrandom::Error> {
        let mut cek = Zeroizing::new([0; 32]);
        getrandom::fill(cek.as_mut_slice())?;
        Ok(Self(cek))
    }

    pub(super) fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl From<[u8; 32]> for Cek {
    fn from(bytes: [u8; 32]) -> Self {
        Self(Zeroizing::new(bytes))
    }
}

/// A content-key wrap, by format.
pub(super) enum Wrap {
    /// PASERK `k4.local-wrap.pie` plus `k4.lid`, as legacy JSON.
    Pie(PieWrap),
    /// Avelope wrap type `0x01`: committing XChaCha20-Poly1305 under the master key.
    Symmetric(SymmetricWrap),
}

impl Wrap {
    /// Classify a wire `cek` string; anything outside the avelope container is legacy JSON.
    fn parse(cek: &str) -> Result<Self, ParseError> {
        let Some(encoded) = cek.strip_prefix(EncryptedData::AVELOPE_PREFIX) else {
            return Ok(Self::Pie(PieWrap::parse(cek)?));
        };
        let blob = URL_SAFE_NO_PAD.decode(encoded).map_err(|source| ShapeError::Base64 {
            layer: Layer::Wrap,
            source,
        })?;
        Self::parse_blob(&blob)
    }

    /// Classify an avelope wrap blob by its type byte.
    pub(super) fn parse_blob(blob: &[u8]) -> Result<Self, ParseError> {
        match blob.first() {
            None => Err(ShapeError::Length {
                layer: Layer::Wrap,
                len: 0,
            }
            .into()),
            Some(&SymmetricWrap::TYPE) => Ok(Self::Symmetric(SymmetricWrap::parse(blob)?)),
            Some(&wrap_type) => Err(UnsupportedFormat::WrapType(wrap_type).into()),
        }
    }

    fn key_id(&self) -> KeyId {
        match self {
            Self::Pie(wrap) => wrap.key_id(),
            Self::Symmetric(wrap) => wrap.key_id(),
        }
    }

    /// The only way to a [`MatchedWrap`]: [`WrongKey`] unless this wrap names `key`.
    pub(super) fn matching(self, key: &Key) -> Result<MatchedWrap<'_>, WrongKey> {
        let found = self.key_id();
        if found != key.key_id() {
            return Err(WrongKey {
                found,
                given: key.key_id(),
            });
        }
        Ok(MatchedWrap {
            wrap: self,
            key,
            proof: KidMatched(()),
        })
    }
}

/// Proof that a wrap's kid matched the key; only [`Wrap::matching`] constructs it.
pub(super) struct KidMatched(());

/// A wrap whose kid is the key's; the only thing that unwraps.
pub(super) struct MatchedWrap<'k> {
    wrap: Wrap,
    key: &'k Key,
    proof: KidMatched,
}

impl MatchedWrap<'_> {
    /// Unwrap the content key, then open `payload` with it.
    pub(super) fn open(self, payload: Payload<'_>, aad: &str) -> Result<Vec<u8>, OpenError> {
        payload.open(&self.unwrap_cek()?, aad)
    }

    /// Unwrap the content key and wrap it under `new` in this wrap's own format, as a `cek` string.
    fn rewrap(self, new: &Key) -> Result<String, RewrapError> {
        match self.wrap {
            Wrap::Pie(wrap) => Ok(PieWrap::seal(&wrap.unwrap_cek(self.key, self.proof)?, new)),
            Wrap::Symmetric(wrap) => {
                let cek = wrap.unwrap_cek(self.key, self.proof).map_err(OpenError::from)?;
                let mut nonce = [0; 24];
                getrandom::fill(&mut nonce).map_err(SealError::from)?;
                Ok(EncryptedData::avelope_text(SymmetricWrap::seal(&cek, new, &nonce)?.as_bytes()))
            }
        }
    }

    fn unwrap_cek(self) -> Result<Cek, OpenError> {
        match self.wrap {
            Wrap::Pie(wrap) => wrap.unwrap_cek(self.key, self.proof),
            Wrap::Symmetric(wrap) => Ok(wrap.unwrap_cek(self.key, self.proof)?),
        }
    }
}

/// A sealed payload, by format.
pub(super) enum Payload<'a> {
    Legacy(LegacyToken<'a>),
    Avelope(AvelopePayload),
}

impl<'a> Payload<'a> {
    /// Classify a wire `data` string.
    fn parse(raw: &'a str) -> Result<Self, ParseError> {
        if raw.starts_with(LegacyToken::PREFIX) {
            return Ok(Self::Legacy(LegacyToken(raw)));
        }
        let Some(encoded) = raw.strip_prefix(EncryptedData::AVELOPE_PREFIX) else {
            return Err(ShapeError::UnknownShape(Layer::Payload).into());
        };
        let blob = URL_SAFE_NO_PAD.decode(encoded).map_err(|source| ShapeError::Base64 {
            layer: Layer::Payload,
            source,
        })?;
        Self::parse_blob(blob)
    }

    /// Classify an avelope payload blob by its version byte.
    pub(super) fn parse_blob(blob: Vec<u8>) -> Result<Self, ParseError> {
        Ok(Self::Avelope(AvelopePayload::parse(blob)?))
    }

    fn open(self, cek: &Cek, aad: &str) -> Result<Vec<u8>, OpenError> {
        match self {
            Self::Legacy(token) => token.open(cek, aad),
            Self::Avelope(payload) => Ok(payload.open(cek, aad)?),
        }
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use rstest::rstest;

    use super::*;
    use crate::encryption::error::{AuthenticationError, MalformedError};
    use crate::encryption::tests::{KAT_AAD, KAT_PLAINTEXT, KAT_WIRE_CEK, key, other_key};

    /// `AvelopePayload::seal(kat_cek, payload nonce, CROSS_AVELOPE_PIE.0, KAT_PLAINTEXT)` over
    /// `PieWrap::seal(kat_cek, key())`: an avelope payload under a legacy wrap.
    const CROSS_AVELOPE_PIE: (&str, &str, &str) = (
        r#"{"id":"00000000-0000-0000-0000-00000000000a","idx":0,"version":"v1","tag":"history","host":"00000000-0000-0000-0000-000000000002"}"#,
        "avelope.AUBBQkNERUZHSElKS0xNTk9QUVJTVFVWV8tLPEOtgV5rrhaAy40ymxfVkh7CrDeCyWogjB46FjSald1eMG0t4h7yDdksb-h9HXmuiZki",
        r#"{"wpk":"k4.local-wrap.pie.KILkQVwLG4CYixPh3poIENlaJ_bWF-X5KLjLLL9iOSfIAJonH9KNncl6tNCGFJ8lgOUGEvT3ONThGdFr2ufsWM6nTejpvTZsrTsyAqTHtS-DBdqOZcXmGUDsoHJ2dw-Q","kid":"k4.lid.2LzCmxDtbwu2tK5T1X1VLEth8umaI9vbKgTDkkt7ARR0"}"#,
    );

    /// `LegacyToken::seal(kat_cek, CROSS_LEGACY_AVELOPE.0, KAT_PLAINTEXT)` over
    /// `SymmetricWrap::seal(kat_cek, key(), wrap nonce)`: a legacy payload under an avelope wrap.
    const CROSS_LEGACY_AVELOPE: (&str, &str, &str) = (
        r#"{"id":"00000000-0000-0000-0000-00000000000b","idx":0,"version":"v1","tag":"history","host":"00000000-0000-0000-0000-000000000002"}"#,
        "v4.local.Qhr7mVG6u6lGbYDOuPigcE6eQvACqhlXoUZCZvDzYVC3gKMX5fM-cRehckgiYPvlro2hv8QmBqiEMBJaM5avOJhPuQxa735gtHzov1gIE2ebEg",
        KAT_WIRE_CEK,
    );

    impl EncryptedData {
        /// A value whose payload and wrap are sealed independently, so the two formats can differ.
        fn seal_pair(
            payload: EnvelopeFormat,
            wrap: EnvelopeFormat,
            key: &Key,
            aad: &str,
            plaintext: &[u8],
        ) -> Self {
            let Draws {
                cek,
                payload_nonce,
                wrap_nonce,
            } = Draws::random().unwrap();
            let raw = match payload {
                EnvelopeFormat::Legacy => LegacyToken::seal(&cek, aad, plaintext).unwrap(),
                EnvelopeFormat::Avelope => Self::avelope_text(
                    &AvelopePayload::seal(&cek, &payload_nonce, aad, plaintext)
                        .unwrap()
                        .into_bytes(),
                ),
            };
            let cek = match wrap {
                EnvelopeFormat::Legacy => PieWrap::seal(&cek, key),
                EnvelopeFormat::Avelope => Self::avelope_text(
                    SymmetricWrap::seal(&cek, key, &wrap_nonce).unwrap().as_bytes(),
                ),
            };
            Self { raw, cek }
        }

        fn with_raw(&self, raw: impl Into<String>) -> Self {
            Self {
                raw: raw.into(),
                cek: self.cek.clone(),
            }
        }

        fn with_cek(&self, cek: impl Into<String>) -> Self {
            Self {
                raw: self.raw.clone(),
                cek: cek.into(),
            }
        }
    }

    #[rstest]
    fn wire_format_is_stable() {
        // The sync wire contract: these JSON field names must stay `data` and
        // `content_encryption_key` regardless of the Rust field names, or old and new
        // clients/servers (which do not upgrade atomically) can no longer exchange records.
        let data = EncryptedData {
            raw: "R".to_owned(),
            cek: "C".to_owned(),
        };
        assert_eq!(
            serde_json::to_string(&data).unwrap(),
            r#"{"data":"R","content_encryption_key":"C"}"#
        );

        let from_wire: EncryptedData =
            serde_json::from_str(r#"{"data":"R","content_encryption_key":"C"}"#).unwrap();
        assert_eq!(from_wire, data);

        // The internal field names are accepted as aliases on the way in.
        let from_alias: EncryptedData = serde_json::from_str(r#"{"raw":"R","cek":"C"}"#).unwrap();
        assert_eq!(from_alias, data);
    }

    #[rstest]
    fn round_trips(
        #[values(EnvelopeFormat::Legacy, EnvelopeFormat::Avelope)] format: EnvelopeFormat,
    ) {
        proptest!(|(aad in "[ -~]{0,64}", plaintext in prop::collection::vec(any::<u8>(), 0..2048))| {
            let key = Key::generate();
            let sealed = EncryptedData::seal_as(format, &key, &aad, &plaintext).unwrap();

            prop_assert_eq!(sealed.open(&key, &aad).unwrap(), plaintext);
        });
    }

    #[rstest]
    #[case::avelope_payload_legacy_wrap(CROSS_AVELOPE_PIE)]
    #[case::legacy_payload_avelope_wrap(CROSS_LEGACY_AVELOPE)]
    fn frozen_cross_pairs_open(key: Key, #[case] pair: (&str, &str, &str)) {
        let (aad, raw, cek) = pair;
        let value = EncryptedData {
            raw: raw.to_owned(),
            cek: cek.to_owned(),
        };

        assert_eq!(value.open(&key, aad).unwrap(), KAT_PLAINTEXT);
        assert_eq!(value.to_bytes(), None);
    }

    #[rstest]
    #[case::bad_base64("avelope.!!", |e: &OpenError| matches!(e, OpenError::Malformed(MalformedError::Shape(ShapeError::Base64 { layer: Layer::Payload, .. }))))]
    #[case::padded("avelope.AQ==", |e: &OpenError| matches!(e, OpenError::Malformed(MalformedError::Shape(ShapeError::Base64 { layer: Layer::Payload, .. }))))]
    #[case::trailing_bits("avelope.AR", |e: &OpenError| matches!(e, OpenError::Malformed(MalformedError::Shape(ShapeError::Base64 { layer: Layer::Payload, .. }))))]
    #[case::empty_blob("avelope.", |e: &OpenError| matches!(e, OpenError::Malformed(MalformedError::Shape(ShapeError::Length { layer: Layer::Payload, len: 0 }))))]
    #[case::short(EncryptedData::avelope_text(&[1; 72]), |e: &OpenError| matches!(e, OpenError::Malformed(MalformedError::Shape(ShapeError::Length { layer: Layer::Payload, len: 72 }))))]
    #[case::newer_version(EncryptedData::avelope_text(&[2; 73]), |e: &OpenError| matches!(e, OpenError::UnsupportedFormat(UnsupportedFormat::Version { layer: Layer::Payload, version: 2 })))]
    #[case::version_zero(EncryptedData::avelope_text(&[0; 73]), |e: &OpenError| matches!(e, OpenError::UnsupportedFormat(UnsupportedFormat::Version { layer: Layer::Payload, version: 0 })))]
    #[case::without_dot("avelope", |e: &OpenError| matches!(e, OpenError::Malformed(MalformedError::Shape(ShapeError::UnknownShape(Layer::Payload)))))]
    #[case::v4_public("v4.public.x", |e: &OpenError| matches!(e, OpenError::Malformed(MalformedError::Shape(ShapeError::UnknownShape(Layer::Payload)))))]
    #[case::empty("", |e: &OpenError| matches!(e, OpenError::Malformed(MalformedError::Shape(ShapeError::UnknownShape(Layer::Payload)))))]
    fn classifies_the_payload(
        key: Key,
        #[case] raw: String,
        #[case] expected: fn(&OpenError) -> bool,
    ) {
        let value = EncryptedData::seal_as(EnvelopeFormat::Avelope, &key, KAT_AAD, b"x")
            .unwrap()
            .with_raw(raw);

        let err = value.open(&key, KAT_AAD).unwrap_err();
        assert!(expected(&err), "{err:?}");
    }

    #[rstest]
    #[case::bad_base64("avelope.!!", |e: &ParseError| matches!(e, ParseError::Malformed(ShapeError::Base64 { layer: Layer::Wrap, .. })))]
    #[case::empty_blob("avelope.", |e: &ParseError| matches!(e, ParseError::Malformed(ShapeError::Length { layer: Layer::Wrap, len: 0 })))]
    #[case::type_0x00(EncryptedData::avelope_text(&[0x00; 138]), |e: &ParseError| matches!(e, ParseError::UnsupportedFormat(UnsupportedFormat::WrapType(0x00))))]
    #[case::type_0x02(EncryptedData::avelope_text(&[0x02; 138]), |e: &ParseError| matches!(e, ParseError::UnsupportedFormat(UnsupportedFormat::WrapType(0x02))))]
    #[case::type_0xff(EncryptedData::avelope_text(&[0xff]), |e: &ParseError| matches!(e, ParseError::UnsupportedFormat(UnsupportedFormat::WrapType(0xff))))]
    #[case::type_0x01_short(EncryptedData::avelope_text(&[0x01; 137]), |e: &ParseError| matches!(e, ParseError::Malformed(ShapeError::Length { layer: Layer::Wrap, len: 137 })))]
    #[case::type_0x01_long(EncryptedData::avelope_text(&[0x01; 139]), |e: &ParseError| matches!(e, ParseError::Malformed(ShapeError::Length { layer: Layer::Wrap, len: 139 })))]
    #[case::empty_json("{}", |e: &ParseError| matches!(e, ParseError::Malformed(ShapeError::LegacyCekJson(_))))]
    #[case::v4("v4.x", |e: &ParseError| matches!(e, ParseError::Malformed(ShapeError::LegacyCekJson(_))))]
    #[case::bad_kid(r#"{"wpk":"k4.local-wrap.pie.KILkQVwLG4CYixPh3poIENlaJ_bWF-X5KLjLLL9iOSfIAJonH9KNncl6tNCGFJ8lgOUGEvT3ONThGdFr2ufsWM6nTejpvTZsrTsyAqTHtS-DBdqOZcXmGUDsoHJ2dw-Q","kid":"k4.lid.bad"}"#, |e: &ParseError| matches!(e, ParseError::Malformed(ShapeError::LegacyKeyId(_))))]
    fn classifies_the_wrap(#[case] cek: String, #[case] expected: fn(&ParseError) -> bool) {
        let value = EncryptedData {
            raw: String::new(),
            cek,
        };

        let err = value.key_id().unwrap_err();
        assert!(expected(&err), "{err:?}");
    }

    #[rstest]
    #[case::avelope("avelope.x", true)]
    #[case::empty_avelope("avelope.", true)]
    #[case::without_dot("avelope", false)]
    #[case::legacy("v4.local.x", false)]
    #[case::garbage("garbage", false)]
    fn is_avelope_reads_the_prefix(#[case] raw: &str, #[case] expected: bool) {
        let value = EncryptedData {
            raw: raw.to_owned(),
            cek: String::new(),
        };

        assert_eq!(value.is_avelope(), expected);
    }

    #[rstest]
    fn checks_the_key_before_the_payload(
        key: Key,
        other_key: Key,
        #[values(EnvelopeFormat::Legacy, EnvelopeFormat::Avelope)] format: EnvelopeFormat,
    ) {
        let theirs = EncryptedData::seal_as(format, &other_key, KAT_AAD, b"x").unwrap();
        let ours = EncryptedData::seal_as(format, &key, KAT_AAD, b"x").unwrap();

        assert!(matches!(
            theirs.with_raw("garbage").open(&key, KAT_AAD),
            Err(OpenError::WrongKey(_))
        ));
        let newer = EncryptedData::avelope_text(&[2; 73]);
        assert!(matches!(theirs.with_raw(&newer).open(&key, KAT_AAD), Err(OpenError::WrongKey(_))));
        assert!(matches!(
            ours.with_raw(&newer).open(&key, KAT_AAD),
            Err(OpenError::UnsupportedFormat(UnsupportedFormat::Version {
                layer: Layer::Payload,
                version: 2
            }))
        ));
        assert!(matches!(
            ours.with_cek(EncryptedData::avelope_text(&[2; 138])).open(&key, KAT_AAD),
            Err(OpenError::UnsupportedFormat(UnsupportedFormat::WrapType(2)))
        ));
    }

    #[rstest]
    #[case::legacy(EnvelopeFormat::Legacy, |e: &OpenError| matches!(e, OpenError::Authentication(AuthenticationError::Legacy { layer: Layer::Payload, .. })))]
    #[case::avelope(EnvelopeFormat::Avelope, |e: &OpenError| matches!(e, OpenError::Authentication(AuthenticationError::Commitment(Layer::Payload))))]
    fn the_wrong_aad_fails_authentication(
        key: Key,
        #[case] format: EnvelopeFormat,
        #[case] expected: fn(&OpenError) -> bool,
    ) {
        let value = EncryptedData::seal_as(format, &key, KAT_AAD, b"x").unwrap();

        let err = value.open(&key, &KAT_AAD.replace("history", "kv")).unwrap_err();
        assert!(expected(&err), "{err:?}");
    }

    #[rstest]
    fn the_wrong_key_keeps_its_text(
        key: Key,
        other_key: Key,
        #[values(EnvelopeFormat::Legacy, EnvelopeFormat::Avelope)] format: EnvelopeFormat,
    ) {
        let value = EncryptedData::seal_as(format, &key, KAT_AAD, b"x").unwrap();

        let err = value.open(&other_key, KAT_AAD).unwrap_err();
        assert!(matches!(err, OpenError::WrongKey(_)), "{err:?}");
        assert_eq!(
            err.to_string(),
            format!(
                "unexpected error decrypting CEK: bad key. encrypted key id: {}, given decryption \
                 key: {}",
                key.key_id(),
                other_key.key_id()
            )
        );
    }

    #[rstest]
    fn rewrap_preserves_the_format(
        #[values(EnvelopeFormat::Legacy, EnvelopeFormat::Avelope)] payload: EnvelopeFormat,
        #[values(EnvelopeFormat::Legacy, EnvelopeFormat::Avelope)] wrap: EnvelopeFormat,
    ) {
        proptest!(|(plaintext in prop::collection::vec(any::<u8>(), 0..512))| {
            let (old, new) = (Key::generate(), Key::generate());
            let value = EncryptedData::seal_pair(payload, wrap, &old, KAT_AAD, &plaintext);

            let rewrapped = value.rewrap(&old, &new).unwrap();
            prop_assert_eq!(&rewrapped.raw, &value.raw);
            prop_assert_eq!(rewrapped.open(&new, KAT_AAD).unwrap(), plaintext);
            let wrong_key = WrongKey { found: new.key_id(), given: old.key_id() };
            let err = rewrapped.open(&old, KAT_AAD).unwrap_err();
            prop_assert!(matches!(err, OpenError::WrongKey(found) if found == wrong_key), "{err:?}");
            match wrap {
                EnvelopeFormat::Legacy => {
                    let pie = rewrapped.cek.starts_with(r#"{"wpk":"k4.local-wrap.pie."#);
                    prop_assert!(pie, "{}", rewrapped.cek);
                }
                EnvelopeFormat::Avelope => {
                    let blob = URL_SAFE_NO_PAD
                        .decode(rewrapped.cek.strip_prefix(EncryptedData::AVELOPE_PREFIX).unwrap())
                        .unwrap();
                    prop_assert_eq!((blob[0], blob.len()), (SymmetricWrap::TYPE, SymmetricWrap::LEN));
                }
            }
        });
    }

    #[rstest]
    fn rewrap_carries_a_newer_payload_but_not_a_newer_wrap(key: Key, other_key: Key) {
        let ours = EncryptedData::seal_as(EnvelopeFormat::Avelope, &key, KAT_AAD, b"x").unwrap();
        let newer = EncryptedData::avelope_text(&[2; 80]);

        let newer_payload = ours.with_raw(&newer).rewrap(&key, &other_key).unwrap();
        assert_eq!(newer_payload.raw, newer);
        assert_eq!(newer_payload.key_id().unwrap(), other_key.key_id());

        assert!(matches!(
            ours.with_cek(EncryptedData::avelope_text(&[0x02; 138])).rewrap(&key, &other_key),
            Err(RewrapError::Open(OpenError::UnsupportedFormat(UnsupportedFormat::WrapType(2))))
        ));
    }

    #[rstest]
    fn key_id_reads_only_the_wrap(
        key: Key,
        #[values(EnvelopeFormat::Legacy, EnvelopeFormat::Avelope)] format: EnvelopeFormat,
    ) {
        let value = EncryptedData::seal_as(format, &key, KAT_AAD, b"x").unwrap();

        assert_eq!(value.key_id().unwrap(), key.key_id());
        assert_eq!(value.with_raw("garbage").key_id().unwrap(), key.key_id());
    }

    #[rstest]
    fn never_panics_on_arbitrary_strings(key: Key, other_key: Key) {
        proptest!(|(raw in any::<String>(), cek in any::<String>())| {
            let value = EncryptedData { raw, cek };

            prop_assert!(value.open(&key, KAT_AAD).is_err(), "{value:?}");
            let _ = value.key_id();
            let _ = value.rewrap(&key, &other_key);
            let _ = value.to_bytes();
        });
    }

    #[rstest]
    fn never_panics_past_the_key_check(
        key: Key,
        #[values(EnvelopeFormat::Legacy, EnvelopeFormat::Avelope)] format: EnvelopeFormat,
        #[values("avelope.", "v4.local.")] prefix: &str,
    ) {
        let ours = EncryptedData::seal_as(format, &key, KAT_AAD, b"x").unwrap();
        proptest!(|(body in prop::collection::vec(any::<u8>(), 0..256), text in "[A-Za-z0-9_.=-]{0,160}")| {
            let encoded = ours.with_raw(format!("{prefix}{}", URL_SAFE_NO_PAD.encode(&body)));
            prop_assert!(encoded.open(&key, KAT_AAD).is_err(), "{encoded:?}");
            let text = ours.with_raw(format!("{prefix}{text}"));
            prop_assert!(text.open(&key, KAT_AAD).is_err(), "{text:?}");
        });
    }
}
