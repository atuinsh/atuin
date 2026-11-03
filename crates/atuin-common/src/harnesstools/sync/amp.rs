//! Amp: none. Amp keeps its threads on its servers, where atuin can't write, so there is no local
//! copy to read a tip from, catch up or switch, and nothing here to say whether it is live.

use std::path::Path;

use super::{
    AppendOptions, AppendOutcome, Liveness, LocalTip, ReplaceOutcome, SessionSync, SyncError,
};
use crate::harnesstools::amp::Amp;
use crate::harnesstools::rehydrate::RehydrateMessage;

/// Why every write is refused.
const AMP_REMOTE: &str = "Amp threads live on Amp's servers";

impl SessionSync for Amp {
    async fn local_tip(&self, _id: &str) -> Result<Option<LocalTip>, SyncError> {
        Err(SyncError::Unsupported(AMP_REMOTE))
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
        Err(SyncError::Unsupported(AMP_REMOTE))
    }

    async fn replace(
        &self,
        _id: &str,
        _base: &LocalTip,
        _branch: &[RehydrateMessage],
        _options: &AppendOptions<'_>,
        _backups: &Path,
    ) -> Result<ReplaceOutcome, SyncError> {
        Err(SyncError::Unsupported(AMP_REMOTE))
    }
}
