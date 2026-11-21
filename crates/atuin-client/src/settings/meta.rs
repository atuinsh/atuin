use atuin_common::path::RelativeTo;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_with::serde_as;

use super::{ConfigDir, DbLocation};

#[serde_as]
#[derive(Debug, Serialize, Deserialize, Clone, JsonSchema)]
pub struct Settings {
    /// The database Atuin keeps its own state in, such as your Hub session.
    #[serde_as(as = "RelativeTo<ConfigDir>")]
    pub db_path: DbLocation,
}
