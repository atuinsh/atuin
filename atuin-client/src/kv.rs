use eyre::Result;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct KvRecord {
    pub key: String,
    pub value: String,
}

impl KvRecord {
    pub fn serialize(&self) -> Result<Vec<u8>> {
        let buf = rmp_serde::to_vec(self)?;

        Ok(buf)
    }
}
