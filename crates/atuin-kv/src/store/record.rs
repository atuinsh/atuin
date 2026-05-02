use atuin_common::record::DecryptedData;
use eyre::{Result, bail, ensure, eyre};
use typed_builder::TypedBuilder;

pub const KV_VERSION: &str = "v1";
pub const KV_TAG: &str = "kv";
pub const KV_VAL_MAX_LEN: usize = 100 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, TypedBuilder)]
pub struct KvRecord {
    pub namespace: String,
    pub key: String,
    pub value: Option<String>,
}

impl KvRecord {
    pub fn serialize(&self) -> Result<DecryptedData> {
        use rmp::encode;

        let mut output = vec![];

        // INFO: ensure this is updated when adding new fields
        encode::write_array_len(&mut output, 4)?;

        encode::write_str(&mut output, &self.namespace)?;
        encode::write_str(&mut output, &self.key)?;
        encode::write_bool(&mut output, self.value.is_some())?;

        if let Some(value) = &self.value {
            encode::write_str(&mut output, value)?;
        }

        Ok(DecryptedData(output))
    }

    pub fn deserialize(data: &DecryptedData, version: &str) -> Result<Self> {
        use rmp::decode;

        fn error_report<E: std::fmt::Debug>(err: E) -> eyre::Report {
            eyre!("{err:?}")
        }

        match version {
            "v0" => {
                let mut bytes = decode::Bytes::new(&data.0);

                let nfields = decode::read_array_len(&mut bytes).map_err(error_report)?;
                ensure!(nfields == 3, "too many entries in v0 kv record");

                let bytes = bytes.remaining_slice();

                let (namespace, bytes) =
                    decode::read_str_from_slice(bytes).map_err(error_report)?;
                let (key, bytes) = decode::read_str_from_slice(bytes).map_err(error_report)?;
                let (value, bytes) = decode::read_str_from_slice(bytes).map_err(error_report)?;

                if !bytes.is_empty() {
                    bail!("trailing bytes in encoded kvrecord. malformed")
                }

                Ok(KvRecord {
                    namespace: namespace.to_owned(),
                    key: key.to_owned(),
                    value: Some(value.to_owned()),
                })
            }
            KV_VERSION => {
                let mut bytes = decode::Bytes::new(&data.0);

                let nfields = decode::read_array_len(&mut bytes).map_err(error_report)?;
                ensure!(nfields == 4, "too many entries in v1 kv record");

                let bytes = bytes.remaining_slice();

                let (namespace, bytes) =
                    decode::read_str_from_slice(bytes).map_err(error_report)?;
                let (key, mut bytes) = decode::read_str_from_slice(bytes).map_err(error_report)?;
                let has_value = decode::read_bool(&mut bytes).map_err(error_report)?;

                let (value, bytes) = if has_value {
                    let (value, bytes) =
                        decode::read_str_from_slice(bytes).map_err(error_report)?;
                    (Some(value.to_owned()), bytes)
                } else {
                    (None, bytes)
                };

                if !bytes.is_empty() {
                    bail!("trailing bytes in encoded kvrecord. malformed")
                }

                Ok(KvRecord {
                    namespace: namespace.to_owned(),
                    key: key.to_owned(),
                    value,
                })
            }
            _ => {
                bail!("unknown version {version:?}")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{DecryptedData, KV_VERSION, KvRecord};

    #[test]
    fn encode_decode_some() {
        let kv = KvRecord {
            namespace: "foo".to_owned(),
            key: "bar".to_owned(),
            value: Some("baz".to_owned()),
        };
        let snapshot = [
            0x94, 0xa3, b'f', b'o', b'o', 0xa3, b'b', b'a', b'r', 0xc3, 0xa3, b'b', b'a', b'z',
        ];

        let encoded = kv.serialize().unwrap();
        let decoded = KvRecord::deserialize(&encoded, KV_VERSION).unwrap();

        assert_eq!(encoded.0, &snapshot);
        assert_eq!(decoded, kv);
    }

    #[test]
    fn encode_decode_none() {
        let kv = KvRecord {
            namespace: "foo".to_owned(),
            key: "bar".to_owned(),
            value: None,
        };
        let snapshot = [0x94, 0xa3, b'f', b'o', b'o', 0xa3, b'b', b'a', b'r', 0xc2];

        let encoded = kv.serialize().unwrap();
        let decoded = KvRecord::deserialize(&encoded, KV_VERSION).unwrap();

        assert_eq!(encoded.0, &snapshot);
        assert_eq!(decoded, kv);
    }

    #[test]
    fn decode_v0() {
        let kv = KvRecord {
            namespace: "foo".to_owned(),
            key: "bar".to_owned(),
            value: Some("baz".to_owned()),
        };

        let snapshot = vec![
            0x93, 0xa3, b'f', b'o', b'o', 0xa3, b'b', b'a', b'r', 0xa3, b'b', b'a', b'z',
        ];

        let decoded = KvRecord::deserialize(&DecryptedData(snapshot), "v0").unwrap();

        assert_eq!(decoded, kv);
    }
}
