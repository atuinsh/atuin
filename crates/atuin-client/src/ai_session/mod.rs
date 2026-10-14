use std::path::PathBuf;

use crate::settings::Settings;

pub mod model;
pub use model::*;
mod store;
pub use store::*;
mod database;
pub use database::*;

/// Where the daemon keeps the ai-session sidecar (see [`Settings::ai_session_sidecar_path`]).
#[must_use]
pub fn sidecar_path(settings: &Settings) -> PathBuf {
    settings.ai_session_sidecar_path()
}

/// Have the daemon reproject the sidecar from the whole record store on its next start, for
/// commands that re-encrypt the records under it (which changes none of what they say).
pub async fn invalidate_sidecar(settings: &Settings) -> Result<(), DbError> {
    AiSessionDatabase::invalidate_projection(sidecar_path(settings)).await
}
