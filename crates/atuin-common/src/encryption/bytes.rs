use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;

use super::avelope::{AvelopePayload, Draws};
use super::envelope::{Payload, Wrap};
use super::error::{OpenError, SealError};
use super::legacy::LegacyPacked;
use super::{EncryptedData, EnvelopeFormat, Key};

/// An [`EncryptedData`] with its base64 and JSON stripped, for storage that holds bytes.
///
/// A `cek` of exactly 129 bytes is a packed legacy value; any other length is an avelope payload
/// and wrap blob pair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncryptedBytes {
    pub data: Vec<u8>,
    pub cek: Vec<u8>,
}

impl EncryptedBytes {
    /// A `cek` this long is a packed legacy value; no avelope wrap is ever this long.
    pub(super) const LEGACY_CEK_LEN: usize = 129;

    /// Seal `plaintext` under a fresh content key wrapped by `key`, in `format`.
    pub fn seal_as(
        format: EnvelopeFormat,
        key: &Key,
        aad: &str,
        plaintext: &[u8],
    ) -> Result<Self, SealError> {
        match format {
            EnvelopeFormat::Legacy => Ok(EncryptedData::seal_legacy(key, aad, plaintext)?
                .to_bytes()
                .expect("a fresh legacy seal is canonical")),
            EnvelopeFormat::Avelope => Self::seal_avelope(key, aad, plaintext, Draws::random()?),
        }
    }

    /// Open a value sealed with `aad`, in any format this build reads.
    pub fn open(self, key: &Key, aad: &str) -> Result<Vec<u8>, OpenError> {
        if self.cek.len() == Self::LEGACY_CEK_LEN {
            return EncryptedData::from(self).open(key, aad);
        }
        // The payload is parsed only once the wrap names this key.
        Wrap::parse_blob(&self.cek)?.matching(key)?.open(Payload::parse_blob(self.data)?, aad)
    }
}

impl EncryptedData {
    /// This value as bytes, if it round-trips through them; `None` means keep the text.
    #[must_use]
    pub fn to_bytes(&self) -> Option<EncryptedBytes> {
        if let Some(packed) = LegacyPacked::pack(self) {
            return Some(packed.into_bytes());
        }

        let decode =
            |text: &str| URL_SAFE_NO_PAD.decode(text.strip_prefix(Self::AVELOPE_PREFIX)?).ok();
        let cek = decode(&self.cek)?;
        // Bytes tell legacy from avelope only by this length, so no wrap type may be stored at it,
        // whatever this build knows about the type.
        if cek.len() == EncryptedBytes::LEGACY_CEK_LEN {
            return None;
        }
        Wrap::parse_blob(&cek).ok()?;
        // Strict base64 makes `From<EncryptedBytes>` rebuild exactly these strings.
        let data = AvelopePayload::parse(decode(&self.raw)?).ok()?.into_bytes();
        Some(EncryptedBytes { data, cek })
    }
}

/// Total: a 129-byte `cek` is a packed legacy value, anything else an avelope blob pair.
impl From<EncryptedBytes> for EncryptedData {
    fn from(EncryptedBytes { data, cek }: EncryptedBytes) -> Self {
        if let Ok(packed) = <&[u8; EncryptedBytes::LEGACY_CEK_LEN]>::try_from(cek.as_slice()) {
            return LegacyPacked::unpack(data, packed);
        }
        Self {
            raw: Self::avelope_text(&data),
            cek: Self::avelope_text(&cek),
        }
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use rstest::rstest;

    use super::*;
    use crate::encryption::Layer;
    use crate::encryption::error::{MalformedError, ShapeError, UnsupportedFormat};
    use crate::encryption::tests::{
        GOLDEN_CEK, GOLDEN_RAW, KAT_AAD, KAT_WIRE_CEK, KAT_WIRE_DATA, key,
    };

    /// The golden record as atuin 18.24 stored it (`paseto_v4::EncryptedBytes`): the token body.
    const GOLDEN_BYTES_DATA: &str = "71216123d9f7d0c7f0919ad102df98028c69e83ac030cc9b98bbabcbbb2f7453246a99b14262d0691cea09f21d6a73da439e556c98c68eac23b2dd809cb881404f5c0100321faf6ed87547ae468abfbac6b5";
    /// The golden record's `wpk` body (96 bytes) then its kid (33 bytes).
    const GOLDEN_BYTES_CEK: &str = "f315cf82b372962114cbf3e76ccdd2f3c624f2d4100341cdda8ea3c949061cae5db9411f5becd2088d64498469c8f11208aefee7cdb684fcd101bc995cf45501b0a4a0baaac0f27cfe88bc725284afabbc9c9b884102c754296f609999c9922ed8bcc29b10ed6f0bb6b4ae53d57d552c4b61f2e99a23dbdb2a04c3924b7b011474";

    /// The avelope KAT's payload blob.
    const KAT_BYTES_DATA: &str = "01404142434445464748494a4b4c4d4e4f505152535455565735d7831a44d6a272a43e69e138fa6764228c4707ae067fff63152bb3032de44424aaeb22636f5497b8e626373df74ed17c419aa9d1";
    /// The avelope KAT's wrap blob.
    const KAT_BYTES_CEK: &str = "01d8bcc29b10ed6f0bb6b4ae53d57d552c4b61f2e99a23dbdb2a04c3924b7b011474808182838485868788898a8b8c8d8e8f9091929394959697308155ce26222c27a999c2261ecad62ea743f5990249099e51e0288489e09a0735c286c06b86b8c797bdfa113bea6a6bfccf4c8f73a25ca84de09f1d8743c4cd89f2ecf36ea8e009467c95438409d78a";

    /// The avelope KAT's wrap blob, decoded from its text.
    fn kat_wrap_blob() -> Vec<u8> {
        URL_SAFE_NO_PAD
            .decode(KAT_WIRE_CEK.strip_prefix(EncryptedData::AVELOPE_PREFIX).unwrap())
            .unwrap()
    }

    #[rstest]
    #[case::legacy(GOLDEN_RAW, GOLDEN_CEK, GOLDEN_BYTES_DATA, GOLDEN_BYTES_CEK)]
    #[case::avelope(KAT_WIRE_DATA, KAT_WIRE_CEK, KAT_BYTES_DATA, KAT_BYTES_CEK)]
    fn has_the_frozen_layout(
        key: Key,
        #[case] raw: &str,
        #[case] cek: &str,
        #[case] data_hex: &str,
        #[case] cek_hex: &str,
    ) {
        let text = EncryptedData {
            raw: raw.to_owned(),
            cek: cek.to_owned(),
        };
        let bytes = EncryptedBytes {
            data: hex::decode(data_hex).unwrap(),
            cek: hex::decode(cek_hex).unwrap(),
        };

        assert_eq!(text.to_bytes().as_ref(), Some(&bytes));
        assert_eq!(EncryptedData::from(bytes.clone()), text);
        assert_eq!(bytes.open(&key, KAT_AAD).unwrap(), [1, 2, 3, 4, 5]);
    }

    #[rstest]
    fn sealed_values_round_trip_smaller(
        #[values(EnvelopeFormat::Legacy, EnvelopeFormat::Avelope)] format: EnvelopeFormat,
    ) {
        proptest!(|(plaintext in prop::collection::vec(any::<u8>(), 0..1024))| {
            let sealed = EncryptedData::seal_as(format, &Key::generate(), KAT_AAD, &plaintext).unwrap();

            let bytes = sealed.to_bytes().unwrap();
            prop_assert!(bytes.data.len() * 4 < sealed.raw.len() * 3, "{} -> {}", sealed.raw.len(), bytes.data.len());
            prop_assert!(bytes.cek.len() * 4 < sealed.cek.len() * 3, "{} -> {}", sealed.cek.len(), bytes.cek.len());
            let expected_cek_len = match format {
                EnvelopeFormat::Legacy => EncryptedBytes::LEGACY_CEK_LEN,
                EnvelopeFormat::Avelope => 138,
            };
            prop_assert_eq!(bytes.cek.len(), expected_cek_len);
            prop_assert_eq!(EncryptedData::from(bytes), sealed);
        });
    }

    #[rstest]
    fn opens_what_it_seals(
        #[values(EnvelopeFormat::Legacy, EnvelopeFormat::Avelope)] format: EnvelopeFormat,
    ) {
        proptest!(ProptestConfig::with_cases(64), |(aad in "[ -~]{0,64}", plaintext in prop::collection::vec(any::<u8>(), 0..2048))| {
            let key = Key::generate();
            let sealed = EncryptedBytes::seal_as(format, &key, &aad, &plaintext).unwrap();

            prop_assert_eq!(&EncryptedData::from(sealed.clone()).open(&key, &aad).unwrap(), &plaintext);
            prop_assert_eq!(sealed.open(&key, &aad).unwrap(), plaintext);
        });
    }

    #[rstest]
    fn packs_only_what_unpacks_to_itself() {
        let avelope_shaped = (
            prop::collection::vec(any::<u8>(), 0..160).prop_map(|mut b| {
                b.insert(0, AvelopePayload::VERSION);
                EncryptedData::avelope_text(&b)
            }),
            prop::collection::vec(any::<u8>(), 137).prop_map(|mut b| {
                b.insert(0, 0x01);
                EncryptedData::avelope_text(&b)
            }),
        );
        let any_strings = (any::<String>(), any::<String>());
        proptest!(|((raw, cek) in prop_oneof![avelope_shaped, any_strings])| {
            let value = EncryptedData { raw, cek };
            if let Some(bytes) = value.to_bytes() {
                prop_assert_eq!(EncryptedData::from(bytes), value);
            }
        });
    }

    #[rstest]
    fn reads_back_the_bytes_it_would_write() {
        proptest!(|(data in prop::collection::vec(any::<u8>(), 0..200), cek in prop::collection::vec(any::<u8>(), 0..200))| {
            let bytes = EncryptedBytes { data, cek };
            if let Some(again) = EncryptedData::from(bytes.clone()).to_bytes() {
                prop_assert_eq!(again, bytes);
            }
        });
    }

    #[rstest]
    fn never_panics_on_arbitrary_bytes(key: Key) {
        proptest!(|(data in prop::collection::vec(any::<u8>(), 0..200), cek in prop::collection::vec(any::<u8>(), 0..200))| {
            let bytes = EncryptedBytes { data, cek };
            let value = EncryptedData::from(bytes.clone());
            prop_assert!(value.open(&key, KAT_AAD).is_err(), "{value:?}");
            prop_assert!(bytes.open(&key, KAT_AAD).is_err());
        });
    }

    #[rstest]
    fn reads_a_newer_payload_as_unsupported(key: Key) {
        let bytes = EncryptedBytes {
            data: vec![0x02; 80],
            cek: kat_wrap_blob(),
        };

        let value = EncryptedData::from(bytes.clone());
        assert!(value.raw.starts_with(EncryptedData::AVELOPE_PREFIX), "{}", value.raw);
        assert_eq!(value.cek, KAT_WIRE_CEK);
        for err in [value.open(&key, KAT_AAD).unwrap_err(), bytes.open(&key, KAT_AAD).unwrap_err()]
        {
            assert!(
                matches!(
                    err,
                    OpenError::UnsupportedFormat(UnsupportedFormat::Version {
                        layer: Layer::Payload,
                        version: 2
                    })
                ),
                "{err:?}"
            );
        }
    }

    #[rstest]
    fn reads_empty_data_as_malformed(key: Key) {
        let value = EncryptedData::from(EncryptedBytes {
            data: Vec::new(),
            cek: kat_wrap_blob(),
        });

        assert!(matches!(
            value.open(&key, KAT_AAD),
            Err(OpenError::Malformed(MalformedError::Shape(ShapeError::Length {
                layer: Layer::Payload,
                len: 0
            })))
        ));
    }

    #[rstest]
    fn reads_a_129_byte_cek_as_legacy() {
        let value = EncryptedData::from(EncryptedBytes {
            data: vec![0; 64],
            cek: vec![0; EncryptedBytes::LEGACY_CEK_LEN],
        });

        assert!(value.raw.starts_with("v4.local."), "{}", value.raw);
        assert!(value.cek.starts_with(r#"{"wpk":"k4.local-wrap.pie."#), "{}", value.cek);
    }

    #[rstest]
    #[case::empty("")]
    #[case::plaintext_manifest("001{\"host\":\"x\"}")]
    #[case::token_with_footer("v4.local.abc.footer")]
    #[case::not_base64("v4.local.not base64!")]
    #[case::non_canonical_base64("v4.local.QR")]
    #[case::cek_wrong_shape(r#"{"wpk":"x","kid":"y"}"#)]
    #[case::newer_payload("avelope.AgICAgIC")]
    #[case::avelope_not_base64("avelope.!!")]
    fn keeps_as_text_what_is_not_canonical(
        key: Key,
        #[values(EnvelopeFormat::Legacy, EnvelopeFormat::Avelope)] format: EnvelopeFormat,
        #[case] value: &str,
    ) {
        let good = EncryptedData::seal_as(format, &key, KAT_AAD, b"ls -la").unwrap();
        let bad_data = EncryptedData {
            raw: value.into(),
            cek: good.cek.clone(),
        };
        let bad_cek = EncryptedData {
            raw: good.raw,
            cek: value.into(),
        };

        assert_eq!(bad_data.to_bytes(), None);
        assert_eq!(bad_cek.to_bytes(), None);
    }

    #[rstest]
    fn keeps_reformatted_cek_json_as_text(key: Key) {
        let good =
            EncryptedData::seal_as(EnvelopeFormat::Legacy, &key, KAT_AAD, b"ls -la").unwrap();
        let json: serde_json::Value = serde_json::from_str(&good.cek).unwrap();

        let reordered = EncryptedData {
            cek: format!("{{\"kid\":{},\"wpk\":{}}}", json["kid"], json["wpk"]),
            ..good
        };

        assert_eq!(reordered.to_bytes(), None);
    }

    #[rstest]
    fn keeps_an_unknown_wrap_type_as_text(key: Key) {
        let good =
            EncryptedData::seal_as(EnvelopeFormat::Avelope, &key, KAT_AAD, b"ls -la").unwrap();

        let unknown = EncryptedData {
            cek: EncryptedData::avelope_text(&[0x02; 138]),
            ..good
        };

        assert_eq!(unknown.to_bytes(), None);
    }
}
