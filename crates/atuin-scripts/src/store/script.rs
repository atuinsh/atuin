use atuin_common::record::DecryptedData;
use eyre::Result;
use uuid::Uuid;

use rmp::encode;
use typed_builder::TypedBuilder;

pub const SCRIPT_VERSION: &str = "v0";
pub const SCRIPT_TAG: &str = "script";
pub const SCRIPT_LEN: usize = 20000; // 20kb max total len

#[derive(Debug, Clone, PartialEq, Eq, TypedBuilder)]
/// A script is a set of commands that can be run, with the specified shebang
pub struct Script {
    /// The id of the script
    #[builder(default = uuid::Uuid::new_v4())]
    pub id: Uuid,

    /// The name of the script
    pub name: String,

    /// The description of the script
    #[builder(default = String::new())]
    pub description: String,

    /// The interpreter of the script
    #[builder(default = String::new())]
    pub shebang: String,

    /// The tags of the script
    #[builder(default = Vec::new())]
    pub tags: Vec<String>,

    /// The script content
    pub script: String,
}

impl Script {
    pub fn serialize(&self) -> Result<DecryptedData> {
        // sort the tags first, to ensure consistent ordering
        let mut tags = self.tags.clone();
        tags.sort();

        let mut output = vec![];

        encode::write_array_len(&mut output, 6)?;
        encode::write_str(&mut output, &self.id.to_string())?;
        encode::write_str(&mut output, &self.name)?;
        encode::write_str(&mut output, &self.description)?;
        encode::write_str(&mut output, &self.shebang)?;
        encode::write_array_len(&mut output, self.tags.len() as u32)?;

        for tag in &tags {
            encode::write_str(&mut output, tag)?;
        }

        encode::write_str(&mut output, &self.script)?;

        Ok(DecryptedData(output))
    }

    pub fn deserialize(bytes: &[u8]) -> Result<Self> {
        use atuin_common::rmp::{Bytes, decode, decode_array_len, expect_array_len, expect_eof};

        let mut bytes = Bytes::new(bytes);

        expect_array_len(&mut bytes, 6)?;

        let id = decode::<String>(&mut bytes)?;
        let name = decode::<String>(&mut bytes)?;
        let description = decode::<String>(&mut bytes)?;
        let shebang = decode::<String>(&mut bytes)?;

        let tags_len = decode_array_len(&mut bytes)?;

        let mut tags = Vec::new();
        for _ in 0..tags_len {
            let tag = decode::<String>(&mut bytes)?;
            tags.push(tag);
        }

        let script = decode::<String>(&mut bytes)?;

        expect_eof(&bytes)?;

        Ok(Script {
            id: Uuid::parse_str(&id)?,
            name,
            description,
            shebang,
            tags,
            script,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_serialize() {
        let script = Script {
            id: uuid::Uuid::parse_str("0195c825a35f7982bdb016168881cbc6").unwrap(),
            name: "test".to_string(),
            description: "test".to_string(),
            shebang: "test".to_string(),
            tags: vec!["test".to_string()],
            script: "test".to_string(),
        };

        let serialized = script.serialize().unwrap();
        assert_eq!(
            serialized.0,
            vec![
                150, 217, 36, 48, 49, 57, 53, 99, 56, 50, 53, 45, 97, 51, 53, 102, 45, 55, 57, 56,
                50, 45, 98, 100, 98, 48, 45, 49, 54, 49, 54, 56, 56, 56, 49, 99, 98, 99, 54, 164,
                116, 101, 115, 116, 164, 116, 101, 115, 116, 164, 116, 101, 115, 116, 145, 164,
                116, 101, 115, 116, 164, 116, 101, 115, 116
            ]
        );
    }

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

        let serialized = script.serialize().unwrap();

        let deserialized = Script::deserialize(&serialized.0).unwrap();

        assert_eq!(script, deserialized);
    }
}
