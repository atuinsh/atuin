use atuin_common::record::DecryptedData;
use eyre::{Result, eyre};
use uuid::Uuid;

use crate::store::script::SCRIPT_VERSION;

use super::script::Script;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScriptRecord {
    Create(Script),
    Update(Script),
    Delete(Uuid),
}

impl ScriptRecord {
    pub fn serialize(&self) -> Result<DecryptedData> {
        use rmp::encode;

        let mut output = vec![];

        match self {
            ScriptRecord::Create(script) => {
                // 0 -> a script create
                encode::write_u8(&mut output, 0)?;

                let bytes = script.serialize()?;

                encode::write_bin(&mut output, &bytes.0)?;
            }

            ScriptRecord::Delete(id) => {
                // 1 -> a script delete
                encode::write_u8(&mut output, 1)?;
                encode::write_str(&mut output, id.to_string().as_str())?;
            }

            ScriptRecord::Update(script) => {
                // 2 -> a script update
                encode::write_u8(&mut output, 2)?;
                let bytes = script.serialize()?;
                encode::write_bin(&mut output, &bytes.0)?;
            }
        };

        Ok(DecryptedData(output))
    }

    pub fn deserialize(data: &DecryptedData, version: &str) -> Result<Self> {
        use rmp::decode;

        fn error_report<E: std::fmt::Debug>(err: E) -> eyre::Report {
            eyre!("{err:?}")
        }

        match version {
            SCRIPT_VERSION => {
                let mut bytes = decode::Bytes::new(&data.0);

                let record_type = decode::read_u8(&mut bytes).map_err(error_report)?;

                match record_type {
                    // create
                    0 => {
                        // written by encode::write_bin above
                        let _ = decode::read_bin_len(&mut bytes).map_err(error_report)?;
                        let script = Script::deserialize(bytes.remaining_slice())?;
                        Ok(ScriptRecord::Create(script))
                    }

                    // delete
                    1 => {
                        let bytes = bytes.remaining_slice();
                        let (id, _) = decode::read_str_from_slice(bytes).map_err(error_report)?;
                        Ok(ScriptRecord::Delete(Uuid::parse_str(id)?))
                    }

                    // update
                    2 => {
                        // written by encode::write_bin above
                        let _ = decode::read_bin_len(&mut bytes).map_err(error_report)?;
                        let script = Script::deserialize(bytes.remaining_slice())?;
                        Ok(ScriptRecord::Update(script))
                    }

                    _ => Err(eyre!("unknown script record type {record_type}")),
                }
            }
            _ => Err(eyre!("unknown version {version:?}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_serialize_create() {
        let script = Script::builder()
            .id(uuid::Uuid::parse_str("0195c825a35f7982bdb016168881cbc6").unwrap())
            .name("test".to_string())
            .description("test".to_string())
            .shebang("test".to_string())
            .tags(vec!["test".to_string()])
            .script("test".to_string())
            .build();

        let record = ScriptRecord::Create(script);

        let serialized = record.serialize().unwrap();

        assert_eq!(
            serialized.0,
            vec![
                204, 0, 196, 65, 150, 217, 36, 48, 49, 57, 53, 99, 56, 50, 53, 45, 97, 51, 53, 102,
                45, 55, 57, 56, 50, 45, 98, 100, 98, 48, 45, 49, 54, 49, 54, 56, 56, 56, 49, 99,
                98, 99, 54, 164, 116, 101, 115, 116, 164, 116, 101, 115, 116, 164, 116, 101, 115,
                116, 145, 164, 116, 101, 115, 116, 164, 116, 101, 115, 116
            ]
        );
    }

    #[test]
    fn test_serialize_delete() {
        let record = ScriptRecord::Delete(
            uuid::Uuid::parse_str("0195c825a35f7982bdb016168881cbc6").unwrap(),
        );

        let serialized = record.serialize().unwrap();

        assert_eq!(
            serialized.0,
            vec![
                204, 1, 217, 36, 48, 49, 57, 53, 99, 56, 50, 53, 45, 97, 51, 53, 102, 45, 55, 57,
                56, 50, 45, 98, 100, 98, 48, 45, 49, 54, 49, 54, 56, 56, 56, 49, 99, 98, 99, 54
            ]
        );
    }

    #[test]
    fn test_serialize_update() {
        let script = Script::builder()
            .id(uuid::Uuid::parse_str("0195c825a35f7982bdb016168881cbc6").unwrap())
            .name(String::from("test"))
            .description(String::from("test"))
            .shebang(String::from("test"))
            .tags(vec![String::from("test"), String::from("test2")])
            .script(String::from("test"))
            .build();

        let record = ScriptRecord::Update(script);

        let serialized = record.serialize().unwrap();

        assert_eq!(
            serialized.0,
            vec![
                204, 2, 196, 71, 150, 217, 36, 48, 49, 57, 53, 99, 56, 50, 53, 45, 97, 51, 53, 102,
                45, 55, 57, 56, 50, 45, 98, 100, 98, 48, 45, 49, 54, 49, 54, 56, 56, 56, 49, 99,
                98, 99, 54, 164, 116, 101, 115, 116, 164, 116, 101, 115, 116, 164, 116, 101, 115,
                116, 146, 164, 116, 101, 115, 116, 165, 116, 101, 115, 116, 50, 164, 116, 101, 115,
                116
            ],
        );
    }

    #[test]
    fn test_serialize_deserialize_create() {
        let script = Script::builder()
            .name("test".to_string())
            .description("test".to_string())
            .shebang("test".to_string())
            .tags(vec!["test".to_string()])
            .script("test".to_string())
            .build();

        let record = ScriptRecord::Create(script);

        let serialized = record.serialize().unwrap();
        let deserialized = ScriptRecord::deserialize(&serialized, SCRIPT_VERSION).unwrap();

        assert_eq!(record, deserialized);
    }

    #[test]
    fn test_serialize_deserialize_delete() {
        let record = ScriptRecord::Delete(
            uuid::Uuid::parse_str("0195c825a35f7982bdb016168881cbc6").unwrap(),
        );

        let serialized = record.serialize().unwrap();
        let deserialized = ScriptRecord::deserialize(&serialized, SCRIPT_VERSION).unwrap();

        assert_eq!(record, deserialized);
    }

    #[test]
    fn test_serialize_deserialize_update() {
        let script = Script::builder()
            .name("test".to_string())
            .description("test".to_string())
            .shebang("test".to_string())
            .tags(vec!["test".to_string()])
            .script("test".to_string())
            .build();

        let record = ScriptRecord::Update(script);

        let serialized = record.serialize().unwrap();
        let deserialized = ScriptRecord::deserialize(&serialized, SCRIPT_VERSION).unwrap();

        assert_eq!(record, deserialized);
    }
}
