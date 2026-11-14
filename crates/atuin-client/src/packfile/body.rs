//! The encrypted body of a packfile.

use atuin_common::encryption::{
    EncryptedBytes, EncryptedData, EnvelopeFormat, Key, OpenError, SealError,
};
use atuin_common::rmp::decode::{self, Bytes, DecodeError};
use atuin_common::rmp::encode::{self, ByteBuf};
use thiserror::Error;

/// An encrypted packfile body, as uploaded to and downloaded from packfile storage.
///
/// A legacy body is the envelope's two strings as msgpack strings. An avelope body is
/// `0x01 || u16be(wrap blob length) || wrap blob || payload blob`; `0x01` is no msgpack string
/// marker, so a release that reads only legacy bodies fails to parse one before any decryption.
#[derive(Debug, Clone, PartialEq, Eq, derive_more::From, derive_more::Into, derive_more::AsRef)]
#[as_ref(forward)]
pub struct PackfileBody(Vec<u8>);

/// Why a packfile body did not open.
#[derive(Debug, Error)]
pub enum PackfileBodyError {
    #[error(transparent)]
    Open(#[from] OpenError),
    #[error("the packfile body is {len} bytes but its framing needs {needed}")]
    Truncated {
        len: usize,
        needed: usize,
    },
    #[error("the legacy packfile body is not two msgpack strings: {0}")]
    LegacyFraming(#[source] DecodeError<'static>),
    #[error(
        "the packfile body was framed by a newer version of atuin (framing {0:#04x}); upgrade \
         atuin to read it"
    )]
    UnsupportedFraming(u8),
    #[error("the packfile body starts with {0:#04x}, which is no packfile framing")]
    UnknownFraming(u8),
}

impl PackfileBody {
    /// The first byte of an avelope body; legacy bodies start with a msgpack str marker instead.
    const AVELOPE_FRAMING: u8 = 0x01;
    /// The framing byte and the big-endian `u16` wrap length.
    const AVELOPE_HEADER_LEN: usize = 3;

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
                let EncryptedData { raw, cek } =
                    EncryptedData::seal_as(format, key, aad, plaintext)?;
                let mut body = ByteBuf::new();
                encode::write_str(&mut body, &raw).expect("writing to a ByteBuf cannot fail");
                encode::write_str(&mut body, &cek).expect("writing to a ByteBuf cannot fail");
                Ok(Self(body.into_vec()))
            }
            EnvelopeFormat::Avelope => {
                let EncryptedBytes { data, cek } =
                    EncryptedBytes::seal_as(format, key, aad, plaintext)?;
                let wrap_len = u16::try_from(cek.len()).expect("a wrap blob is far below 64 KiB");

                let mut body =
                    Vec::with_capacity(Self::AVELOPE_HEADER_LEN + cek.len() + data.len());
                body.push(Self::AVELOPE_FRAMING);
                body.extend_from_slice(&wrap_len.to_be_bytes());
                body.extend_from_slice(&cek);
                body.extend_from_slice(&data);
                Ok(Self(body))
            }
        }
    }

    /// Open a body sealed with `aad`, in any format this build reads.
    pub fn open(&self, key: &Key, aad: &str) -> Result<Vec<u8>, PackfileBodyError> {
        let body = self.0.as_slice();
        let Some(&framing) = body.first() else {
            return Err(PackfileBodyError::Truncated { len: 0, needed: 1 });
        };
        match framing {
            // The msgpack str markers `write_str` starts a legacy body with; no single bit flip
            // turns the avelope framing byte into one of these.
            0xa0..=0xbf | 0xd9..=0xdb => {
                let mut bytes = Bytes::new(body);
                let mut read = || {
                    decode::read_string(&mut bytes)
                        .map_err(|err| PackfileBodyError::LegacyFraming(err.into_static()))
                };
                // Trailing bytes are ignored, as every release has.
                let value = EncryptedData {
                    raw: read()?,
                    cek: read()?,
                };
                Ok(value.open(key, aad)?)
            }
            Self::AVELOPE_FRAMING => self.open_avelope(key, aad),
            0x02..=0x9f => Err(PackfileBodyError::UnsupportedFraming(framing)),
            _ => Err(PackfileBodyError::UnknownFraming(framing)),
        }
    }

    fn open_avelope(&self, key: &Key, aad: &str) -> Result<Vec<u8>, PackfileBodyError> {
        let len = self.0.len();
        let Some((wrap_len, rest)) = self.0.get(1..).and_then(<[u8]>::split_first_chunk::<2>)
        else {
            return Err(PackfileBodyError::Truncated {
                len,
                needed: Self::AVELOPE_HEADER_LEN,
            });
        };
        let wrap_len = usize::from(u16::from_be_bytes(*wrap_len));
        let Some((wrap, payload)) = rest.split_at_checked(wrap_len) else {
            return Err(PackfileBodyError::Truncated {
                len,
                needed: Self::AVELOPE_HEADER_LEN + wrap_len,
            });
        };
        let bytes = EncryptedBytes {
            data: payload.to_vec(),
            cek: wrap.to_vec(),
        };
        Ok(bytes.open(key, aad)?)
    }
}

#[cfg(test)]
mod tests {
    use atuin_common::encryption::{
        AuthenticationError, Layer, MalformedError, ShapeError, UnsupportedFormat,
    };
    use proptest::prelude::*;
    use rstest::{fixture, rstest};

    use super::*;

    /// A frozen `PackIA` JSON.
    const KAT_PACK_AAD: &str = r#"{"manifest_id":"00000000-0000-0000-0000-000000000003","manifest_idx":4,"manifest_version":"v1","host":"00000000-0000-0000-0000-000000000002","tag":"packfile"}"#;

    /// `packfile body` sealed in avelope by the reference implementation, under [`key`], with the
    /// encryption KATs' content key and nonces.
    const KAT_BODY: &str = "01008a01d8bcc29b10ed6f0bb6b4ae53d57d552c4b61f2e99a23dbdb2a04c3924b7b011474808182838485868788898a8b8c8d8e8f9091929394959697308155ce26222c27a999c2261ecad62ea743f5990249099e51e0288489e09a0735c286c06b86b8c797bdfa113bea6a6bfccf4c8f73a25ca84de09f1d8743c4cd89f2ecf36ea8e009467c95438409d78a01404142434445464748494a4b4c4d4e4f50515253545556575d573663ad8714636e5cdaa083940a5f915f844e5f92732ee9d1d1ee61eb7875f34cadf4583763cfb20679c2ea6ba34af4ecafcb8fa2342e2228ba4f83";

    /// `paseto_v4::encrypt_sync` over `packfile body`, framed as two msgpack strings.
    const LEGACY_BODY: &str = "d98576342e6c6f63616c2e667137696f4641744c764534423735306b384566354d3856765f65756f6b50775578625f53636f616e556c6f4f4c53547354555438356f5f6f645978635843556a744e6a4239696e7a46486e6f35462d6965484a586f66784432374c3962782d5343516569356d726a724572387575314430504e416939505a6c4649d9d87b2277706b223a226b342e6c6f63616c2d777261702e7069652e5f6f3051755a7337455744753578695442364162596e786c346c4839325a6e7a665649764158635735347244354f48787172785a4d754a2d392d3437684e4574526e43566f5a6a35476f742d5f4f5f366f5931556d39554c576957616537743659756d394671764b38426e6e4247772d6f444c5f4a513978326241536d4c5858222c226b6964223a226b342e6c69642e324c7a436d78447462777532744b3554315831564c45746838756d61493976624b6754446b6b743741525230227d";

    /// `paseto_v4::encrypt_sync` over `0..=255` repeated to 70,000 bytes, framed as two msgpack
    /// strings: a str32 token.
    const LEGACY_LARGE_BODY: &[u8] =
        include_bytes!("../../tests/fixtures/packfile/legacy-body-70000.bin");

    /// The wrap blob inside [`KAT_BODY`].
    const KAT_WRAP: std::ops::Range<usize> = 3..141;

    #[fixture]
    fn key() -> Key {
        Key::from([0x55; 32])
    }

    #[fixture]
    fn other_key() -> Key {
        Key::from([0x56; 32])
    }

    impl PackfileBody {
        fn from_hex(hex: &str) -> Self {
            Self(hex::decode(hex).unwrap())
        }
    }

    #[rstest]
    #[case::avelope(PackfileBody::from_hex(KAT_BODY), b"packfile body".to_vec())]
    #[case::legacy(PackfileBody::from_hex(LEGACY_BODY), b"packfile body".to_vec())]
    #[case::legacy_large(PackfileBody(LEGACY_LARGE_BODY.to_vec()), (0..=u8::MAX).cycle().take(70_000).collect())]
    fn opens_frozen_bodies(key: Key, #[case] body: PackfileBody, #[case] plaintext: Vec<u8>) {
        assert_eq!(body.open(&key, KAT_PACK_AAD).unwrap(), plaintext);
    }

    #[rstest]
    fn seals_the_frozen_avelope_framing(key: Key) {
        let PackfileBody(body) =
            PackfileBody::seal_as(EnvelopeFormat::Avelope, &key, KAT_PACK_AAD, b"packfile body")
                .unwrap();
        let kat = hex::decode(KAT_BODY).unwrap();

        assert_eq!(body.len(), kat.len());
        assert_eq!(body[..KAT_WRAP.start + 34], kat[..KAT_WRAP.start + 34]);
        let blobs = EncryptedBytes {
            data: body[KAT_WRAP.end..].to_vec(),
            cek: body[KAT_WRAP].to_vec(),
        };
        assert_eq!(blobs.open(&key, KAT_PACK_AAD).unwrap(), b"packfile body");
    }

    #[rstest]
    fn round_trips(
        #[values(EnvelopeFormat::Legacy, EnvelopeFormat::Avelope)] format: EnvelopeFormat,
    ) {
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
    #[case::framing_only(vec![0x01], 1, 3)]
    #[case::half_a_length(vec![0x01, 0x00], 2, 3)]
    #[case::no_wrap(vec![0x01, 0x00, 0x8a], 3, 141)]
    fn a_truncated_body_is_malformed(
        key: Key,
        #[case] bytes: Vec<u8>,
        #[case] len: usize,
        #[case] needed: usize,
    ) {
        let err = PackfileBody(bytes).open(&key, KAT_PACK_AAD).unwrap_err();

        assert!(
            matches!(err, PackfileBodyError::Truncated { len: l, needed: n } if (l, n) == (len, needed)),
            "{err:?}"
        );
    }

    #[rstest]
    #[case::newer_framing(0x02, |e: &PackfileBodyError| matches!(e, PackfileBodyError::UnsupportedFraming(0x02)))]
    #[case::last_reserved(0x9f, |e: &PackfileBodyError| matches!(e, PackfileBodyError::UnsupportedFraming(0x9f)))]
    #[case::zero(0x00, |e: &PackfileBodyError| matches!(e, PackfileBodyError::UnknownFraming(0x00)))]
    #[case::nil(0xc0, |e: &PackfileBodyError| matches!(e, PackfileBodyError::UnknownFraming(0xc0)))]
    #[case::legacy_garbage(0xd9, |e: &PackfileBodyError| matches!(e, PackfileBodyError::LegacyFraming(_)))]
    fn classifies_the_framing(
        key: Key,
        #[case] framing: u8,
        #[case] expected: fn(&PackfileBodyError) -> bool,
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
            Err(PackfileBodyError::Open(OpenError::WrongKey(_)))
        ));
    }

    #[rstest]
    fn every_flipped_bit_fails_its_field_check(key: Key) {
        let original = hex::decode(KAT_BODY).unwrap();
        proptest!(|(offset in 0..original.len(), bit in 0..8u8)| {
            let mut bytes = original.clone();
            bytes[offset] ^= 1 << bit;

            let err = PackfileBody(bytes).open(&key, KAT_PACK_AAD).unwrap_err();
            let open = |check: fn(&OpenError) -> bool| matches!(&err, PackfileBodyError::Open(e) if check(e));
            let expected = match offset {
                0 => matches!(err, PackfileBodyError::UnsupportedFraming(_) | PackfileBodyError::UnknownFraming(_)),
                1..3 => matches!(err, PackfileBodyError::Truncated { .. })
                    || open(|e| matches!(e, OpenError::Malformed(MalformedError::Shape(ShapeError::Length { layer: Layer::Wrap, .. })))),
                3 => open(|e| matches!(e, OpenError::UnsupportedFormat(UnsupportedFormat::WrapType(_)))),
                4..37 => open(|e| matches!(e, OpenError::WrongKey(_))),
                37..93 => open(|e| matches!(e, OpenError::Authentication(AuthenticationError::Commitment(Layer::Wrap)))),
                93..141 => open(|e| matches!(e, OpenError::Authentication(AuthenticationError::Tag(Layer::Wrap)))),
                141 => open(|e| matches!(e, OpenError::UnsupportedFormat(UnsupportedFormat::Version { layer: Layer::Payload, .. }))),
                142..198 => open(|e| matches!(e, OpenError::Authentication(AuthenticationError::Commitment(Layer::Payload)))),
                _ => open(|e| matches!(e, OpenError::Authentication(AuthenticationError::Tag(Layer::Payload)))),
            };
            prop_assert!(expected, "offset {offset}: {err:?}");
        });
    }

    #[rstest]
    fn never_panics(key: Key) {
        let kat = hex::decode(KAT_BODY).unwrap();
        proptest!(|(bytes in prop::collection::vec(any::<u8>(), 0..512))| {
            let _ = PackfileBody(bytes.clone()).open(&key, KAT_PACK_AAD);

            let mut past_the_wrap = kat[..KAT_WRAP.end].to_vec();
            past_the_wrap.extend_from_slice(&bytes);
            prop_assert!(PackfileBody(past_the_wrap).open(&key, KAT_PACK_AAD).is_err());
        });
    }
}
