use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;

use super::EncryptedData;
use super::envelope::Wrap;
use super::legacy::LegacyPacked;
use super::v5::PayloadV5;

/// A record's `data` and `cek` cells when stored as BLOBs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredBlobs {
    pub data: Vec<u8>,
    pub cek: Vec<u8>,
}

impl StoredBlobs {
    /// A BLOB `cek` this long is a packed legacy value; a versioned value is never stored with one.
    pub(super) const LEGACY_CEK_LEN: usize = 129;
}

impl EncryptedData {
    /// The BLOB cells for this value, if it round-trips through them; `None` means store TEXT.
    #[must_use]
    pub fn to_blobs(&self) -> Option<StoredBlobs> {
        if let Some(packed) = LegacyPacked::pack(self) {
            return Some(packed.into_blobs());
        }

        let cek = URL_SAFE_NO_PAD.decode(self.cek.strip_prefix(Wrap::CONTAINER_PREFIX)?).ok()?;
        // Reading tells legacy from versioned only by this length, so no wrap type may be stored
        // at it, whatever this build knows about the type.
        if cek.len() == StoredBlobs::LEGACY_CEK_LEN {
            return None;
        }
        Wrap::parse_blob(&cek).ok()?;
        // Strict base64 and the version byte checked by `parse` make `From<StoredBlobs>` rebuild
        // exactly this `raw`.
        let data = URL_SAFE_NO_PAD.decode(self.raw.strip_prefix(PayloadV5::WIRE_PREFIX)?).ok()?;
        let data = PayloadV5::parse(data).ok()?.into_bytes();
        Some(StoredBlobs { data, cek })
    }
}

/// Total: a 129-byte `cek` is a packed legacy value, anything else a versioned blob pair.
impl From<StoredBlobs> for EncryptedData {
    fn from(StoredBlobs { data, cek }: StoredBlobs) -> Self {
        if let Ok(packed) = <&[u8; StoredBlobs::LEGACY_CEK_LEN]>::try_from(cek.as_slice()) {
            return LegacyPacked::unpack(data, packed);
        }
        let raw = match data.first() {
            Some(version) => format!("v{version}.{}", URL_SAFE_NO_PAD.encode(&data)),
            None => String::new(),
        };
        Self {
            raw,
            cek: Wrap::wire(&cek),
        }
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use rstest::rstest;

    use super::*;
    use crate::encryption::error::{MalformedError, OpenError, ShapeError, UnsupportedFormat};
    use crate::encryption::tests::{
        GOLDEN_CEK, GOLDEN_RAW, KAT_AAD, KAT_WIRE_CEK, KAT_WIRE_DATA, key,
    };
    use crate::encryption::{EnvelopeFormat, Key, Layer};

    /// The golden record as atuin 18.24 stored it (`paseto_v4::EncryptedBytes`): the token body.
    const GOLDEN_BLOB_DATA: &str = "71216123d9f7d0c7f0919ad102df98028c69e83ac030cc9b98bbabcbbb2f7453246a99b14262d0691cea09f21d6a73da439e556c98c68eac23b2dd809cb881404f5c0100321faf6ed87547ae468abfbac6b5";
    /// The golden record's `wpk` body (96 bytes) then its kid (33 bytes).
    const GOLDEN_BLOB_CEK: &str = "f315cf82b372962114cbf3e76ccdd2f3c624f2d4100341cdda8ea3c949061cae5db9411f5becd2088d64498469c8f11208aefee7cdb684fcd101bc995cf45501b0a4a0baaac0f27cfe88bc725284afabbc9c9b884102c754296f609999c9922ed8bcc29b10ed6f0bb6b4ae53d57d552c4b61f2e99a23dbdb2a04c3924b7b011474";

    /// The v5 KAT's payload blob.
    const KAT_BLOB_DATA: &str = "05404142434445464748494a4b4c4d4e4f50515253545556570021ad2462e85fb6f21ab32734bc2107516c802bf5cff68857869ad5ff4e69f95583dd763c2e1267ec7bf568178b5dcf5a9f402fe0";
    /// The v5 KAT's wrap blob.
    const KAT_BLOB_CEK: &str = "01d8bcc29b10ed6f0bb6b4ae53d57d552c4b61f2e99a23dbdb2a04c3924b7b011474808182838485868788898a8b8c8d8e8f9091929394959697a08e186bb24d1fcbc00d4297394185ea0c524c9bd6c55b74e7782521346c646a0cce40a9f3c2518292ccfdbccbb1b8bb5e774f12d18485b3350c5741108c5d40beeb8395754c8812c21ddcb9a2061cbb";

    #[rstest]
    #[case::legacy(GOLDEN_RAW, GOLDEN_CEK, GOLDEN_BLOB_DATA, GOLDEN_BLOB_CEK)]
    #[case::v5(KAT_WIRE_DATA, KAT_WIRE_CEK, KAT_BLOB_DATA, KAT_BLOB_CEK)]
    fn stores_the_frozen_layout(
        #[case] raw: &str,
        #[case] cek: &str,
        #[case] data_hex: &str,
        #[case] cek_hex: &str,
    ) {
        let wire = EncryptedData {
            raw: raw.to_owned(),
            cek: cek.to_owned(),
        };
        let blobs = StoredBlobs {
            data: hex::decode(data_hex).unwrap(),
            cek: hex::decode(cek_hex).unwrap(),
        };

        assert_eq!(wire.to_blobs().as_ref(), Some(&blobs));
        assert_eq!(EncryptedData::from(blobs), wire);
    }

    #[rstest]
    fn sealed_values_round_trip_smaller(
        #[values(EnvelopeFormat::Legacy, EnvelopeFormat::V5)] format: EnvelopeFormat,
    ) {
        proptest!(|(plaintext in prop::collection::vec(any::<u8>(), 0..1024))| {
            let sealed = EncryptedData::seal_as(format, &Key::generate(), KAT_AAD, &plaintext).unwrap();

            let blobs = sealed.to_blobs().unwrap();
            prop_assert!(blobs.data.len() * 4 < sealed.raw.len() * 3, "{} -> {}", sealed.raw.len(), blobs.data.len());
            prop_assert!(blobs.cek.len() * 4 < sealed.cek.len() * 3, "{} -> {}", sealed.cek.len(), blobs.cek.len());
            let expected_cek_len = match format {
                EnvelopeFormat::Legacy => StoredBlobs::LEGACY_CEK_LEN,
                EnvelopeFormat::V5 => 138,
            };
            prop_assert_eq!(blobs.cek.len(), expected_cek_len);
            prop_assert_eq!(EncryptedData::from(blobs), sealed);
        });
    }

    #[rstest]
    fn packs_only_what_unpacks_to_itself() {
        let v5_shaped = (
            prop::collection::vec(any::<u8>(), 0..160).prop_map(|mut b| {
                b.insert(0, 0x05);
                format!("v5.{}", URL_SAFE_NO_PAD.encode(b))
            }),
            prop::collection::vec(any::<u8>(), 137).prop_map(|mut b| {
                b.insert(0, 0x01);
                Wrap::wire(&b)
            }),
        );
        let any_strings = (any::<String>(), any::<String>());
        proptest!(|((raw, cek) in prop_oneof![v5_shaped, any_strings])| {
            let value = EncryptedData { raw, cek };
            if let Some(blobs) = value.to_blobs() {
                prop_assert_eq!(EncryptedData::from(blobs), value);
            }
        });
    }

    #[rstest]
    fn reads_back_the_blobs_it_would_write() {
        proptest!(|(data in prop::collection::vec(any::<u8>(), 0..200), cek in prop::collection::vec(any::<u8>(), 0..200))| {
            let blobs = StoredBlobs { data, cek };
            if let Some(again) = EncryptedData::from(blobs.clone()).to_blobs() {
                prop_assert_eq!(again, blobs);
            }
        });
    }

    #[rstest]
    fn never_panics_on_arbitrary_blobs(key: Key) {
        proptest!(|(data in prop::collection::vec(any::<u8>(), 0..200), cek in prop::collection::vec(any::<u8>(), 0..200))| {
            let value = EncryptedData::from(StoredBlobs { data, cek });
            prop_assert!(value.open(&key, KAT_AAD).is_err(), "{value:?}");
        });
    }

    #[rstest]
    #[case::newer_payload(6, |e: &OpenError| matches!(e, OpenError::UnsupportedFormat(UnsupportedFormat::Version { layer: Layer::Payload, version: 6 })))]
    #[case::older_than_v5(3, |e: &OpenError| matches!(e, OpenError::Malformed(MalformedError::Shape(ShapeError::UnknownShape(Layer::Payload)))))]
    fn reads_a_versioned_blob_as_its_wire_form(
        key: Key,
        #[case] version: u8,
        #[case] expected: fn(&OpenError) -> bool,
    ) {
        let cek = URL_SAFE_NO_PAD.decode(KAT_WIRE_CEK.strip_prefix("v5.").unwrap()).unwrap();

        let value = EncryptedData::from(StoredBlobs {
            data: vec![version; 80],
            cek,
        });

        assert!(value.raw.starts_with(&format!("v{version}.")), "{}", value.raw);
        assert_eq!(value.cek, KAT_WIRE_CEK);
        let err = value.open(&key, KAT_AAD).unwrap_err();
        assert!(expected(&err), "{err:?}");
    }

    #[rstest]
    fn reads_empty_data_as_malformed(key: Key) {
        let cek = URL_SAFE_NO_PAD.decode(KAT_WIRE_CEK.strip_prefix("v5.").unwrap()).unwrap();

        let value = EncryptedData::from(StoredBlobs {
            data: Vec::new(),
            cek,
        });

        assert!(matches!(
            value.open(&key, KAT_AAD),
            Err(OpenError::Malformed(MalformedError::Shape(ShapeError::UnknownShape(
                Layer::Payload
            ))))
        ));
    }

    #[rstest]
    fn reads_a_129_byte_cek_as_legacy() {
        let value = EncryptedData::from(StoredBlobs {
            data: vec![0; 64],
            cek: vec![0; StoredBlobs::LEGACY_CEK_LEN],
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
    #[case::newer_payload("v6.AAAA")]
    #[case::v5_not_base64("v5.!!")]
    fn stores_as_text_what_is_not_canonical(
        key: Key,
        #[values(EnvelopeFormat::Legacy, EnvelopeFormat::V5)] format: EnvelopeFormat,
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

        assert_eq!(bad_data.to_blobs(), None);
        assert_eq!(bad_cek.to_blobs(), None);
    }

    #[rstest]
    fn stores_reformatted_cek_json_as_text(key: Key) {
        let good =
            EncryptedData::seal_as(EnvelopeFormat::Legacy, &key, KAT_AAD, b"ls -la").unwrap();
        let json: serde_json::Value = serde_json::from_str(&good.cek).unwrap();

        let reordered = EncryptedData {
            cek: format!("{{\"kid\":{},\"wpk\":{}}}", json["kid"], json["wpk"]),
            ..good
        };

        assert_eq!(reordered.to_blobs(), None);
    }

    #[rstest]
    fn stores_an_unknown_wrap_type_as_text(key: Key) {
        let good = EncryptedData::seal_as(EnvelopeFormat::V5, &key, KAT_AAD, b"ls -la").unwrap();

        let unknown = EncryptedData {
            cek: Wrap::wire(&[0x02; 138]),
            ..good
        };

        assert_eq!(unknown.to_blobs(), None);
    }
}
