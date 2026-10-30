use super::aead::PayloadDomain;
use super::envelope::{Payload, Wrap};
use super::error::{MalformedError, OpenError, SealError, ShapeError, UnsupportedFormat};
use super::v5::{Draws, PayloadV5, SymmetricWrap};
use super::{EncryptedData, EnvelopeFormat, Key, Layer};
use crate::rmp::decode::{self, Bytes};
use crate::rmp::encode::{self, ByteBuf};

/// An encrypted packfile body, as uploaded to and downloaded from packfile storage.
#[derive(Debug, Clone, PartialEq, Eq, derive_more::From, derive_more::Into, derive_more::AsRef)]
#[as_ref(forward)]
pub struct PackfileBody(Vec<u8>);

impl PackfileBody {
    /// The first byte of a v5 body; legacy bodies start with a msgpack str marker instead.
    const V5_FRAMING: u8 = 0x05;
    /// The framing byte and the big-endian `u16` wrap length.
    const V5_HEADER_LEN: usize = 3;

    /// Seal `plaintext` as a packfile body under `key`, in [`EnvelopeFormat::WRITE`].
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
            EnvelopeFormat::Legacy => {
                let EncryptedData { raw, cek } = EncryptedData::seal_legacy(key, aad, plaintext)?;
                let mut body = ByteBuf::new();
                encode::write_str(&mut body, &raw).expect("writing to a ByteBuf cannot fail");
                encode::write_str(&mut body, &cek).expect("writing to a ByteBuf cannot fail");
                Ok(Self(body.into_vec()))
            }
            EnvelopeFormat::V5 => Self::seal_v5(key, aad, plaintext, Draws::random()?),
        }
    }

    /// Open a body sealed with `aad`, in any format this build reads.
    pub fn open(&self, key: &Key, aad: &str) -> Result<Vec<u8>, OpenError> {
        let body = self.0.as_slice();
        let Some(&framing) = body.first() else {
            return Err(MalformedError::BodyTruncated { len: 0, needed: 1 }.into());
        };
        match framing {
            // The msgpack str markers `write_str` starts a legacy body with; `0x05` is a positive
            // fixint, and no single bit flip turns it into one of these.
            0xa0..=0xbf | 0xd9..=0xdb => {
                let mut bytes = Bytes::new(body);
                let mut read = || {
                    decode::read_string(&mut bytes)
                        .map_err(|err| MalformedError::BodyFraming(err.into_static()))
                };
                // Trailing bytes are ignored, as every release has.
                let value = EncryptedData {
                    raw: read()?,
                    cek: read()?,
                };
                value.open_as(key, PayloadDomain::Packfile, aad)
            }
            Self::V5_FRAMING => self.open_v5(key, aad),
            0x06..=0x9f => Err(UnsupportedFormat::Version {
                layer: Layer::Body,
                version: framing,
            }
            .into()),
            _ => Err(MalformedError::Shape(ShapeError::UnknownShape(Layer::Body)).into()),
        }
    }

    fn open_v5(&self, key: &Key, aad: &str) -> Result<Vec<u8>, OpenError> {
        let len = self.0.len();
        let Some((wrap_len, rest)) = self.0.get(1..).and_then(<[u8]>::split_first_chunk::<2>)
        else {
            return Err(MalformedError::BodyTruncated {
                len,
                needed: Self::V5_HEADER_LEN,
            }
            .into());
        };
        let wrap_len = usize::from(u16::from_be_bytes(*wrap_len));
        let Some((wrap, payload)) = rest.split_at_checked(wrap_len) else {
            return Err(MalformedError::BodyTruncated {
                len,
                needed: Self::V5_HEADER_LEN + wrap_len,
            }
            .into());
        };
        // The payload is parsed only once the wrap names this key.
        Wrap::parse_blob(wrap)?.matching(key)?.open(
            Payload::parse_blob(payload.to_vec())?,
            PayloadDomain::Packfile,
            aad,
        )
    }

    /// Seal with v5 framing and `draws`.
    pub(super) fn seal_v5(
        key: &Key,
        aad: &str,
        plaintext: &[u8],
        Draws {
            cek,
            payload_nonce,
            wrap_nonce,
        }: Draws,
    ) -> Result<Self, SealError> {
        let wrap = SymmetricWrap::seal(&cek, key, &wrap_nonce)?;
        let payload =
            PayloadV5::seal(&cek, PayloadDomain::Packfile, &payload_nonce, aad, plaintext)?
                .into_bytes();
        let wrap = wrap.as_bytes();
        let wrap_len = u16::try_from(wrap.len()).expect("a wrap blob is far below 64 KiB");

        let mut body = Vec::with_capacity(Self::V5_HEADER_LEN + wrap.len() + payload.len());
        body.push(Self::V5_FRAMING);
        body.extend_from_slice(&wrap_len.to_be_bytes());
        body.extend_from_slice(wrap);
        body.extend_from_slice(&payload);
        Ok(Self(body))
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use rstest::rstest;

    use super::*;
    use crate::encryption::error::AuthenticationError;
    use crate::encryption::tests::{KAT_PACK_AAD, kat_draws, key, other_key};

    const KAT_BODY: &str = "05008a01d8bcc29b10ed6f0bb6b4ae53d57d552c4b61f2e99a23dbdb2a04c3924b7b011474808182838485868788898a8b8c8d8e8f9091929394959697a08e186bb24d1fcbc00d4297394185ea0c524c9bd6c55b74e7782521346c646a0cce40a9f3c2518292ccfdbccbb1b8bb5e774f12d18485b3350c5741108c5d40beeb8395754c8812c21ddcb9a2061cbb05404142434445464748494a4b4c4d4e4f5051525354555657b7e5167da88a4dfca32deb1bd0fb0782b3d9588778db2cb1a4ad679d66b1b517802a50628ac2ebee31e2a47dc98684627dee419787d48696d40bc19dd6";

    /// `paseto_v4::encrypt_sync` over `packfile body`, framed as two msgpack strings.
    const LEGACY_BODY: &str = "d98576342e6c6f63616c2e667137696f4641744c764534423735306b384566354d3856765f65756f6b50775578625f53636f616e556c6f4f4c53547354555438356f5f6f645978635843556a744e6a4239696e7a46486e6f35462d6965484a586f66784432374c3962782d5343516569356d726a724572387575314430504e416939505a6c4649d9d87b2277706b223a226b342e6c6f63616c2d777261702e7069652e5f6f3051755a7337455744753578695442364162596e786c346c4839325a6e7a665649764158635735347244354f48787172785a4d754a2d392d3437684e4574526e43566f5a6a35476f742d5f4f5f366f5931556d39554c576957616537743659756d394671764b38426e6e4247772d6f444c5f4a513978326241536d4c5858222c226b6964223a226b342e6c69642e324c7a436d78447462777532744b3554315831564c45746838756d61493976624b6754446b6b743741525230227d";

    /// `paseto_v4::encrypt_sync` over `0..=255` repeated to 70,000 bytes, framed as two msgpack
    /// strings: a str32 token.
    const LEGACY_LARGE_BODY: &[u8] =
        include_bytes!("../../tests/fixtures/encryption/legacy-packfile-body-70000.bin");

    impl PackfileBody {
        fn from_hex(hex: &str) -> Self {
            Self(hex::decode(hex).unwrap())
        }
    }

    #[rstest]
    fn seals_the_kat(key: Key, kat_draws: Draws) {
        let sealed =
            PackfileBody::seal_v5(&key, KAT_PACK_AAD, b"packfile body", kat_draws).unwrap();

        assert_eq!(hex::encode(Vec::from(sealed)), KAT_BODY);
    }

    #[rstest]
    #[case::v5(PackfileBody::from_hex(KAT_BODY), b"packfile body".to_vec())]
    #[case::legacy(PackfileBody::from_hex(LEGACY_BODY), b"packfile body".to_vec())]
    #[case::legacy_large(PackfileBody(LEGACY_LARGE_BODY.to_vec()), (0..=u8::MAX).cycle().take(70_000).collect())]
    fn opens_frozen_bodies(key: Key, #[case] body: PackfileBody, #[case] plaintext: Vec<u8>) {
        assert_eq!(body.open(&key, KAT_PACK_AAD).unwrap(), plaintext);
    }

    #[rstest]
    fn round_trips(#[values(EnvelopeFormat::Legacy, EnvelopeFormat::V5)] format: EnvelopeFormat) {
        proptest!(ProptestConfig::with_cases(64), |(plaintext in prop::collection::vec(any::<u8>(), 0..4096))| {
            let key = Key::generate();
            let sealed = PackfileBody::seal_as(format, &key, KAT_PACK_AAD, &plaintext).unwrap();

            prop_assert_eq!(sealed.open(&key, KAT_PACK_AAD).unwrap(), plaintext);
        });
    }

    #[rstest]
    #[case::str8(0, 0xd9)]
    #[case::str16(1_000, 0xda)]
    #[case::str32(70_000, 0xdb)]
    fn opens_every_legacy_string_marker(key: Key, #[case] len: usize, #[case] marker: u8) {
        let plaintext = vec![0x2a; len];

        let sealed =
            PackfileBody::seal_as(EnvelopeFormat::Legacy, &key, KAT_PACK_AAD, &plaintext).unwrap();

        assert_eq!(sealed.0[0], marker);
        assert_eq!(sealed.open(&key, KAT_PACK_AAD).unwrap(), plaintext);
    }

    #[rstest]
    #[case::empty(vec![], 0, 1)]
    #[case::framing_only(vec![0x05], 1, 3)]
    #[case::half_a_length(vec![0x05, 0x00], 2, 3)]
    #[case::no_wrap(vec![0x05, 0x00, 0x8a], 3, 141)]
    fn a_truncated_body_is_malformed(
        key: Key,
        #[case] bytes: Vec<u8>,
        #[case] len: usize,
        #[case] needed: usize,
    ) {
        let err = PackfileBody(bytes).open(&key, KAT_PACK_AAD).unwrap_err();

        assert!(
            matches!(err, OpenError::Malformed(MalformedError::BodyTruncated { len: l, needed: n }) if (l, n) == (len, needed)),
            "{err:?}"
        );
    }

    #[rstest]
    #[case::newer_framing(0x06, |e: &OpenError| matches!(e, OpenError::UnsupportedFormat(UnsupportedFormat::Version { layer: Layer::Body, version: 0x06 })))]
    #[case::last_reserved(0x9f, |e: &OpenError| matches!(e, OpenError::UnsupportedFormat(UnsupportedFormat::Version { layer: Layer::Body, version: 0x9f })))]
    #[case::zero(0x00, |e: &OpenError| matches!(e, OpenError::Malformed(MalformedError::Shape(ShapeError::UnknownShape(Layer::Body)))))]
    #[case::nil(0xc0, |e: &OpenError| matches!(e, OpenError::Malformed(MalformedError::Shape(ShapeError::UnknownShape(Layer::Body)))))]
    #[case::legacy_garbage(0xd9, |e: &OpenError| matches!(e, OpenError::Malformed(MalformedError::BodyFraming(_))))]
    fn classifies_the_framing(
        key: Key,
        #[case] framing: u8,
        #[case] expected: fn(&OpenError) -> bool,
    ) {
        let mut bytes = hex::decode(KAT_BODY).unwrap();
        bytes[0] = framing;

        let err = PackfileBody(bytes).open(&key, KAT_PACK_AAD).unwrap_err();
        assert!(expected(&err), "{err:?}");
    }

    #[rstest]
    fn a_body_under_another_key_is_the_wrong_key(other_key: Key) {
        assert!(matches!(
            PackfileBody::from_hex(KAT_BODY).open(&other_key, KAT_PACK_AAD),
            Err(OpenError::WrongKey(_))
        ));
    }

    #[rstest]
    fn every_flipped_bit_fails_its_field_check(key: Key) {
        let original = hex::decode(KAT_BODY).unwrap();
        proptest!(|(offset in 0..original.len(), bit in 0..8u8)| {
            let mut bytes = original.clone();
            bytes[offset] ^= 1 << bit;

            let err = PackfileBody(bytes).open(&key, KAT_PACK_AAD).unwrap_err();
            let expected = match offset {
                0 => matches!(
                    err,
                    OpenError::UnsupportedFormat(UnsupportedFormat::Version { layer: Layer::Body, .. })
                        | OpenError::Malformed(MalformedError::Shape(ShapeError::UnknownShape(Layer::Body)))
                ),
                1..3 => matches!(
                    err,
                    OpenError::Malformed(
                        MalformedError::BodyTruncated { .. }
                            | MalformedError::Shape(ShapeError::Length { layer: Layer::Wrap, .. })
                    )
                ),
                3 => matches!(err, OpenError::UnsupportedFormat(UnsupportedFormat::WrapType(_))),
                4..37 => matches!(err, OpenError::WrongKey(_)),
                37..93 => matches!(err, OpenError::Authentication(AuthenticationError::Commitment(Layer::Wrap))),
                93..141 => matches!(err, OpenError::Authentication(AuthenticationError::Tag(Layer::Wrap))),
                141 => matches!(
                    err,
                    OpenError::UnsupportedFormat(UnsupportedFormat::Version { layer: Layer::Payload, .. })
                        | OpenError::Malformed(MalformedError::Shape(ShapeError::UnknownShape(Layer::Payload)))
                ),
                142..198 => matches!(err, OpenError::Authentication(AuthenticationError::Commitment(Layer::Payload))),
                _ => matches!(err, OpenError::Authentication(AuthenticationError::Tag(Layer::Payload))),
            };
            prop_assert!(expected, "offset {offset}: {err:?}");
        });
    }

    #[rstest]
    fn never_panics(key: Key, kat_draws: Draws) {
        let wrap = SymmetricWrap::seal(&kat_draws.cek, &key, &kat_draws.wrap_nonce).unwrap();
        proptest!(|(bytes in prop::collection::vec(any::<u8>(), 0..512))| {
            let _ = PackfileBody(bytes.clone()).open(&key, KAT_PACK_AAD);

            let mut past_the_wrap = vec![0x05, 0x00, 0x8a];
            past_the_wrap.extend_from_slice(wrap.as_bytes());
            past_the_wrap.extend_from_slice(&bytes);
            prop_assert!(PackfileBody(past_the_wrap).open(&key, KAT_PACK_AAD).is_err());
        });
    }
}
