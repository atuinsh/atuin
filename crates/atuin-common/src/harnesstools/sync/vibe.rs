//! Mistral Vibe keeps its sessions in a store private to its runtime (see `vibe::session`), which
//! atuin reads but never writes. A session atuin writes for Vibe is a legacy session folder of its
//! own (see `vibe::rehydrate`), which Vibe imports into its store when it is resumed: until then,
//! catching the session up appends to that folder, and once Vibe has imported it, the session is
//! Vibe's, so the caller forks instead.

use std::collections::{HashMap, HashSet};
use std::num::NonZeroUsize;
use std::path::Path;

use futures::TryStreamExt as _;

use super::liveness::flock_holder;
use super::{
    AppendOptions, AppendOutcome, Liveness, LocalTip, Processes, ReplaceOutcome, SessionSync,
    Stamp, SyncError, blocking, blocking_liveness, require_idle,
};
use crate::harnesstools::rehydrate::RehydrateMessage;
use crate::harnesstools::session::{Message as _, Session as _};
use crate::harnesstools::vibe::session::{VibeSession, legacy_folder, store_dir, written_rows};
use crate::harnesstools::vibe::{Vibe, rehydrate, session_root};
use crate::path::PathBufExt as _;
use crate::sync::BlockingPool;

const PRIVATE: &str = "Vibe's session store can't be written to";

impl SessionSync for Vibe {
    /// The rows of the session as capture reads them from Vibe's store (with the rows atuin wrote
    /// to a legacy folder Vibe imported it from); or, before Vibe has imported that folder, the
    /// rows atuin wrote to it.
    async fn local_tip(&self, id: &str) -> Result<Option<LocalTip>, SyncError> {
        let root = session_root();
        let (found, legacy) = {
            let (root, id) = (root.clone(), id.to_owned());
            blocking(move || Ok((store_dir(&root, &id), legacy_folder(&root, &id)))).await?
        };
        let written = legacy.as_deref().and_then(written_rows);
        if legacy.is_some() && written.is_none() {
            return Err(SyncError::Unsupported("a Vibe 2 session isn't read by atuin"));
        }
        let (mut known, mut tip, mut cwd) = match written {
            Some((rows, cwd)) => {
                (rows.iter().cloned().collect::<HashSet<_>>(), rows.last().cloned(), cwd)
            }
            None => (HashSet::new(), None, None),
        };
        let Some(dir) = found else {
            let Some(folder) = legacy else {
                return Ok(None);
            };
            let transcript = folder.join("messages.jsonl");
            let bytes = tokio::fs::read(&transcript).await?;
            let modified =
                tokio::fs::metadata(&transcript).await.ok().and_then(|m| m.modified().ok());
            return Ok(Some(LocalTip {
                native_path: folder,
                known_source_ids: known,
                merged: HashMap::new(),
                tip_source_id: tip,
                stamp: Stamp::of(&bytes),
                modified,
                cwd,
                unswitchable: Some(PRIVATE),
            }));
        };
        // Vibe imported the folder: its rows are those the reader takes from it, which may be
        // fewer than were written, when some were appended as Vibe imported it.
        known.clear();
        tip = None;
        let pool = BlockingPool::new(NonZeroUsize::MIN);
        let Some(session) = VibeSession::open(&root, dir.clone(), pool) else {
            return Ok(None);
        };
        let rows: Vec<_> =
            session.read().try_collect().await.map_err(|e| SyncError::Other(e.to_string()))?;
        for row in &rows {
            if let Some(row_id) = row.id() {
                known.insert(row_id.to_string());
                // A title row is the envelope's, no row Vibe goes on from.
                if row.title().is_none() {
                    tip = Some(row_id.to_string());
                }
            }
            cwd = row.cwd().or(cwd);
        }
        let current = tokio::fs::read(dir.join("CURRENT")).await?;
        let modified = {
            let dir = dir.clone();
            tokio::task::spawn_blocking(move || crate::fs::newest_modified(dir))
                .await
                .ok()
                .flatten()
        };
        Ok(Some(LocalTip {
            native_path: dir,
            known_source_ids: known,
            merged: HashMap::new(),
            tip_source_id: tip,
            stamp: Stamp::of(&current),
            modified,
            cwd,
            unswitchable: Some(PRIVATE),
        }))
    }

    /// Whether a Vibe process holds the session's lease: an exclusive `flock` on
    /// `active/<session>.lock` under the session root, held while the session is open.
    async fn is_live(&self, id: &str, _cwd: Option<&Path>) -> Liveness {
        let (root, id) = (session_root(), id.to_owned());
        blocking_liveness(move || {
            let native = store_dir(&root, &id)
                .and_then(|dir| dir.file_name().map(|name| name.to_string_lossy().into_owned()))
                .unwrap_or(id);
            let lock = root.joined("active").joined(format!("{native}.lock"));
            flock_holder(&lock, &Processes::here())
        })
        .await
    }

    /// Appends to the legacy folder atuin wrote, while Vibe hasn't imported it yet.
    async fn append(
        &self,
        id: &str,
        base: &LocalTip,
        lines: &[RehydrateMessage],
        options: &AppendOptions<'_>,
    ) -> Result<AppendOutcome, SyncError> {
        require_idle(self.is_live(id, options.cwd).await)?;
        let (root, id, base, lines) = (session_root(), id.to_owned(), base.clone(), lines.to_vec());
        blocking(move || {
            if store_dir(&root, &id).is_some() {
                return Err(SyncError::Unsupported(PRIVATE));
            }
            let folder = legacy_folder(&root, &id).ok_or(SyncError::NotFound)?;
            if folder != base.native_path {
                return Err(SyncError::Changed);
            }
            let appended = rehydrate::append(&folder, base.stamp, &lines)?;
            Ok(AppendOutcome {
                native_path: folder,
                tip_source_id: appended.last().cloned(),
                appended,
            })
        })
        .await
    }

    async fn replace(
        &self,
        _id: &str,
        _base: &LocalTip,
        _branch: &[RehydrateMessage],
        _options: &AppendOptions<'_>,
        _backups: &Path,
    ) -> Result<ReplaceOutcome, SyncError> {
        Err(SyncError::Unsupported(PRIVATE))
    }
}
