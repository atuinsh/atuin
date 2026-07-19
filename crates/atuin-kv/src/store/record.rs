use atuin_common::record::DecryptedData;
use atuin_common::rmp::{Bytes, decode, expect_array_len, expect_eof};
use eyre::{Result, bail};
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
        match version {
            "v0" => {
                let mut bytes = Bytes::new(&data.0);

                expect_array_len(&mut bytes, 3)?;

                let namespace = decode::<String>(&mut bytes)?;
                let key = decode::<String>(&mut bytes)?;
                let value = decode::<String>(&mut bytes)?;

                expect_eof(&bytes)?;

                Ok(KvRecord {
                    namespace,
                    key,
                    value: Some(value),
                })
            }
            KV_VERSION => {
                let mut bytes = Bytes::new(&data.0);

                expect_array_len(&mut bytes, 4)?;

                let namespace = decode::<String>(&mut bytes)?;
                let key = decode::<String>(&mut bytes)?;
                let has_value = decode::<bool>(&mut bytes)?;

                let value = if has_value {
                    Some(decode::<String>(&mut bytes)?)
                } else {
                    None
                };

                expect_eof(&bytes)?;

                Ok(KvRecord {
                    namespace,
                    key,
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
