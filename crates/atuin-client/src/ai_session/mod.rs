use std::path::PathBuf;

use crate::settings::Settings;

pub mod model;
pub use model::*;
mod store;
pub use store::*;
mod database;
pub use database::*;

/// Where the daemon keeps the ai-session sidecar.
#[must_use]
pub fn sidecar_path() -> PathBuf {
    Settings::effective_data_dir().join("ai_harness_sessions.db")
}

/// Have the daemon reproject the sidecar from the whole record store on its next start, for
/// commands that rewrite the record store under it.
pub async fn invalidate_sidecar() -> Result<(), DbError> {
    AiSessionDatabase::invalidate_projection(sidecar_path()).await
}
