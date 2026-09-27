//! Writing a Codex session back out from captured messages.
//!
//! A stub: the Codex rehydration workstream replaces this file.

use std::path::PathBuf;

use crate::harnesstools::rehydrate::{RehydrateError, RehydrateSession};

#[allow(clippy::unused_async)]
pub async fn rehydrate(_session: &RehydrateSession) -> Result<PathBuf, RehydrateError> {
    Err(RehydrateError::Unsupported("Codex"))
}
