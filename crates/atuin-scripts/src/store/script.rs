use uuid::Uuid;
use std::collections::BTreeMap;

use atuin_client::record::sqlite_store::SqliteStore;
use atuin_common::record::{DecryptedData, Host, HostId};
use eyre::{Result, bail, ensure, eyre};

use atuin_client::record::encryption::PASETO_V4;
use atuin_client::record::store::Store;
use rmp::{decode::{self, read_str, Bytes}, encode};

const SCRIPT_VERSION: &str = "v0";
const SCRIPT_TAG: &str = "script";
const SCRIPT_LEN: usize = 20000; // 20kb max total len, way more than should be needed.

#[derive(Debug, Clone, PartialEq, Eq)]
/// A script is a set of commands that can be run, with the specified shebang
pub struct Script {
    /// The id of the script
    pub id: Uuid,

    /// The name of the script
    pub name: String,

    /// The description of the script
    pub description: String,

    /// The interpreter of the script
    pub shebang: String,

    /// The tags of the script
    pub tags: Vec<String>,

    /// The script content
    pub script: String,
}

impl Script {
    pub fn serialize(&self, output: &mut Vec<u8>) -> Result<()> {
        encode::write_array_len(output, 6)?;
        encode::write_str(output, &self.id.to_string())?;
        encode::write_str(output, &self.name)?;
        encode::write_str(output, &self.description)?;
        encode::write_str(output, &self.shebang)?;
        encode::write_array_len(output, self.tags.len() as u32)?;

        for tag in &self.tags {
            encode::write_str(output, tag)?;
        }

        encode::write_str(output, &self.script)?;

        Ok(())
    }

    pub fn deserialize(bytes: &mut decode::Bytes) -> Result<Self> {
        fn error_report<E: std::fmt::Debug>(err: E) -> eyre::Report {
            eyre!("{err:?}")
        }

        let nfields = decode::read_array_len(bytes).map_err(error_report)?;

        ensure!(
            nfields == 6,
            "too many entries in v0 script record"
        );

        let bytes = bytes.remaining_slice();

        let (id, bytes) = decode::read_str_from_slice(bytes).map_err(error_report)?;
        let (name, bytes) = decode::read_str_from_slice(bytes).map_err(error_report)?;
        let (description, bytes) = decode::read_str_from_slice(bytes).map_err(error_report)?;
        let (shebang, bytes) = decode::read_str_from_slice(bytes).map_err(error_report)?;

        let mut bytes = Bytes::new(bytes);
        let tags_len = decode::read_array_len(&mut bytes).map_err(error_report)?;

        let mut bytes = bytes.remaining_slice();

        let mut tags = Vec::new();
        for _ in 0..tags_len {
            let (tag, remaining) = decode::read_str_from_slice(bytes).map_err(error_report)?;
            tags.push(tag.to_owned());
            bytes = remaining;
        }

        let (script, bytes) = decode::read_str_from_slice(bytes).map_err(error_report)?;

        if !bytes.is_empty() {
            bail!("trailing bytes in encoded script record. malformed")
        }

        Ok(Script {
            id: Uuid::parse_str(id).map_err(error_report)?,
            name: name.to_owned(),
            description: description.to_owned(),
            shebang: shebang.to_owned(),
            tags: tags.into_iter().map(|s| s.to_owned()).collect(),
            script: script.to_owned(),
        })
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_serialize_deserialize() {
        let script = Script {
            id: uuid::Uuid::new_v4(),
            name: "test".to_string(),
            description: "test".to_string(),
            shebang: "test".to_string(),
            tags: vec!["test".to_string()],
            script: "test".to_string(),
        };

        let mut output = vec![];
        script.serialize(&mut output).unwrap();

        let deserialized = Script::deserialize(&mut decode::Bytes::new(&output)).unwrap();

        assert_eq!(script, deserialized);
    }
}
