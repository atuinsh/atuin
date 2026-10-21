use atuin_common::path::AbsPathBuf;
use serde::{Deserialize, Serialize};
use serde_with::serde_as;

use super::DbFile;

#[serde_as]
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Settings {
    #[serde_as(as = "DbFile")]
    pub db_path: AbsPathBuf,
}
