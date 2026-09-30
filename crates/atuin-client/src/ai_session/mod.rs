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
pub fn sidecar_path() -> PathBuf {
    Settings::ai_session_sidecar_path()
}

/// Have the daemon reproject the sidecar from the whole record store on its next start, for
/// commands that re-encrypt the records under it (which changes none of what they say).
pub async fn invalidate_sidecar() -> Result<(), DbError> {
    AiSessionDatabase::invalidate_projection(sidecar_path()).await
}

/// Delete everything projected into the sidecar, so the daemon rebuilds it from the record store
/// alone, for commands that delete records under it or ask for a rebuild: see
/// [`AiSessionDatabase::reset_projection`]. The daemon's own rebuild (asked over gRPC) does this
/// too, and replays at once; this is for when it cannot be asked.
pub async fn reset_sidecar() -> Result<(), DbError> {
    AiSessionDatabase::reset_projection(sidecar_path()).await
}
