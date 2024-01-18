use eyre::{bail, eyre, Result};
use rmp::decode::Bytes;
use rmp::encode::RmpWrite;
use uuid::Uuid;

pub const CONFIG_ALIAS_VERSION: &str = "v0";
pub const CONFIG_ALIAS_TAG: &str = "config-alias";

use atuin_common::record::{DecryptedData, Host, HostId};

#[derive(Debug, Eq, PartialEq, Clone)]
pub struct AliasId(Uuid);

#[derive(Debug, Eq, PartialEq, Clone)]
pub struct Alias {
    pub id: AliasId,
    pub name: String,
    pub definition: String,
}

#[derive(Debug, Eq, PartialEq, Clone)]
pub enum AliasRecord {
    Create(Alias),   // Create a history record
    Delete(AliasId), // Delete a history record, identified by ID
}

impl AliasRecord {
    pub fn serialize(&self) -> Result<DecryptedData> {
        use rmp::encode;

        let mut output = vec![];

        // write a 0u8 for create, and a 1u8 for delete
        // followed by the data
        // we don't need to write an explicit version in here, because
        // 1. it's stored in the overall record
        // 2. we write the field count anyways
        match self {
            AliasRecord::Create(alias) => {
                // write the type
                encode::write_u8(&mut output, 0)?;

                // write how many fields we are writing
                encode::write_array_len(&mut output, 3)?;

                // write the fields
                let (most, least) = alias.id.0.as_u64_pair();
                encode::write_u64(&mut output, most)?;
                encode::write_u64(&mut output, least)?;

                encode::write_str(&mut output, &alias.name)?;
                encode::write_str(&mut output, &alias.definition)?;
            }
            AliasRecord::Delete(id) => {
                // write the type
                encode::write_u8(&mut output, 1)?;

                // write how many fields we are writing
                encode::write_array_len(&mut output, 1)?;

                let (most, least) = id.0.as_u64_pair();
                encode::write_u64(&mut output, most)?;
                encode::write_u64(&mut output, least)?;
            }
        }

        Ok(DecryptedData(output))
    }

    pub fn deserialize(data: &DecryptedData, version: &str) -> Result<AliasRecord> {
        use rmp::decode;

        if version != CONFIG_ALIAS_VERSION {
            bail!("Invalid version for AliasRecord::deserialize");
        }

        fn error_report<E: std::fmt::Debug>(err: E) -> eyre::Report {
            eyre!("{err:?}")
        }

        let mut bytes = Bytes::new(&data.0);

        // read the type
        let record_type = decode::read_u8(&mut bytes).map_err(error_report)?;

        // read the number of fields
        let field_count = decode::read_array_len(&mut bytes).map_err(error_report)?;

        match record_type {
            0 => {
                // create
                if field_count != 3 {
                    return Err(eyre::eyre!("Invalid field count for AliasRecord::Create"));
                }

                let most = decode::read_u64(&mut bytes).map_err(error_report)?;
                let least = decode::read_u64(&mut bytes).map_err(error_report)?;
                let id = Uuid::from_u64_pair(most, least);

                let bytes = bytes.remaining_slice();
                let (name, bytes) = decode::read_str_from_slice(bytes).map_err(error_report)?;
                let (definition, bytes) =
                    decode::read_str_from_slice(bytes).map_err(error_report)?;

                if !bytes.is_empty() {
                    bail!("trailing bytes in encoded history. malformed")
                }

                let alias = Alias {
                    id: AliasId(id),
                    name: name.to_string(),
                    definition: definition.to_string(),
                };

                Ok(AliasRecord::Create(alias))
            }
            1 => {
                // delete
                if field_count != 1 {
                    return Err(eyre::eyre!("Invalid field count for AliasRecord::Delete"));
                }

                let most = decode::read_u64(&mut bytes).map_err(error_report)?;
                let least = decode::read_u64(&mut bytes).map_err(error_report)?;
                let id = Uuid::from_u64_pair(most, least);

                let alias = AliasId(id);

                Ok(AliasRecord::Delete(alias))
            }
            _ => Err(eyre::eyre!("Invalid record type")),
        }
    }
}

#[cfg(test)]
mod tests {
    use atuin_common::record::DecryptedData;

    use super::{Alias, AliasRecord};

    #[test]
    fn test_encode_alias_create() {
        let snapshot = &[
            204, 0, 147, 207, 1, 141, 29, 204, 218, 86, 127, 99, 207, 185, 95, 141, 153, 16, 161,
            141, 155, 164, 116, 101, 115, 116, 164, 116, 101, 115, 116,
        ];

        // write a test for encoding an alias
        let alias = Alias {
            id: super::AliasId(uuid::uuid!("018d1dccda567f63b95f8d9910a18d9b")),
            name: "test".to_string(),
            definition: "test".to_string(),
        };
        let alias_record = super::AliasRecord::Create(alias);

        let encoded = alias_record.serialize().expect("failed to encode alias");

        assert_eq!(encoded.0.len(), 31);
        assert_eq!(encoded.0, snapshot);
    }

    #[test]
    fn test_decode_alias_create() {
        let snapshot = vec![
            204, 0, 147, 207, 1, 141, 29, 204, 218, 86, 127, 99, 207, 185, 95, 141, 153, 16, 161,
            141, 155, 164, 116, 101, 115, 116, 164, 116, 101, 115, 116,
        ];

        let alias = Alias {
            id: super::AliasId(uuid::uuid!("018d1dccda567f63b95f8d9910a18d9b")),
            name: "test".to_string(),
            definition: "test".to_string(),
        };

        let decoded = AliasRecord::deserialize(&DecryptedData(snapshot), "v0")
            .expect("failed to decode alias");

        assert_eq!(decoded, AliasRecord::Create(alias));
    }

    #[test]
    fn test_alias_encode_decode_create() {
        let alias = Alias {
            id: super::AliasId(uuid::uuid!("018d1dccda567f63b95f8d9910a18d9b")),
            name: "test".to_string(),
            definition: "test".to_string(),
        };

        let create = AliasRecord::Create(alias.clone());

        let encoded = create.serialize().expect("failed to serialize");

        let decoded = AliasRecord::deserialize(&encoded, "v0").expect("failed to deserialize");

        assert_eq!(create, decoded);
    }

    #[test]
    fn test_alias_encode_delete() {
        let snapshot = &[
            204, 1, 145, 207, 1, 141, 29, 227, 138, 189, 120, 83, 207, 140, 223, 145, 3, 114, 55,
            127, 126,
        ];

        let record = AliasRecord::Delete(super::AliasId(uuid::uuid!(
            "018d1de38abd78538cdf910372377f7e"
        )));

        let encoded = record.serialize().expect("failed to serialize");

        assert_eq!(encoded.0.len(), 21);
        assert_eq!(encoded.0, snapshot);
    }

    #[test]
    fn test_alias_decode_delete() {
        let snapshot = vec![
            204, 1, 145, 207, 1, 141, 29, 227, 138, 189, 120, 83, 207, 140, 223, 145, 3, 114, 55,
            127, 126,
        ];
        let record = AliasRecord::Delete(super::AliasId(uuid::uuid!(
            "018d1de38abd78538cdf910372377f7e"
        )));

        let decoded = AliasRecord::deserialize(&DecryptedData(snapshot), "v0")
            .expect("failed to decode alias");

        assert_eq!(decoded, record);
    }

    #[test]
    fn test_alias_encode_decode_delete() {
        let delete = AliasRecord::Delete(super::AliasId(uuid::uuid!(
            "018d1dccda567f63b95f8d9910a18d9b"
        )));

        let encoded = delete.serialize().expect("failed to serialize");

        let decoded = AliasRecord::deserialize(&encoded, "v0").expect("failed to deserialize");

        assert_eq!(delete, decoded);
    }
}
