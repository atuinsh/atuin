use atuin_common::path::RelativeTo;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_with::serde_as;

use super::{ConfigDir, DbLocation};

#[serde_as]
#[derive(Debug, Serialize, Deserialize, Clone, JsonSchema)]
pub struct Settings {
    /// The database `atuin scripts` stores its scripts in.
    #[serde_as(as = "RelativeTo<ConfigDir>")]
    pub db_path: DbLocation,
}
