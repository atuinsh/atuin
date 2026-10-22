use atuin_common::path::RelativeTo;
use serde::{Deserialize, Serialize};
use serde_with::serde_as;

use super::{ConfigDir, DbLocation};

#[serde_as]
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Settings {
    #[serde_as(as = "RelativeTo<ConfigDir>")]
    pub db_path: DbLocation,
}
