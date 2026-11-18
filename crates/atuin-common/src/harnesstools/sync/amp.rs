//! Amp keeps its threads on its servers: there is no local copy for Atuin to read or write.

use std::path::Path;

use super::{
    AppendOptions, AppendOutcome, Liveness, LocalTip, ReplaceOutcome, SessionSync, SyncError,
};
use crate::harnesstools::amp::Amp;
use crate::harnesstools::rehydrate::RehydrateMessage;

const REMOTE: &str = "Amp threads live on Amp's servers";

impl SessionSync for Amp {
    async fn local_tip(&self, _id: &str) -> Result<Option<LocalTip>, SyncError> {
        Err(SyncError::Unsupported(REMOTE))
    }

    async fn is_live(&self, _id: &str, _cwd: Option<&Path>) -> Liveness {
        Liveness::Unknown
    }

    async fn append(
        &self,
        _id: &str,
        _base: &LocalTip,
        _lines: &[RehydrateMessage],
        _options: &AppendOptions<'_>,
    ) -> Result<AppendOutcome, SyncError> {
        Err(SyncError::Unsupported(REMOTE))
    }

    async fn replace(
        &self,
        _id: &str,
        _base: &LocalTip,
        _branch: &[RehydrateMessage],
        _options: &AppendOptions<'_>,
        _backups: &Path,
    ) -> Result<ReplaceOutcome, SyncError> {
        Err(SyncError::Unsupported(REMOTE))
    }
}
