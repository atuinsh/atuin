use std::path::PathBuf;

use crate::settings::Settings;

pub mod model;
pub use model::*;
mod store;
pub use store::*;
mod database;
pub use database::*;

/// The sidecar's name before threads; [`migrate_sidecar_name`] moves it.
const LEGACY_SIDECAR: &str = "ai_session_sidecar.db";

/// Where the daemon keeps the threads sidecar (see [`Settings::threads_db_path`]). Until a daemon
/// has run [`migrate_sidecar_name`], that is still the legacy file.
#[must_use]
pub fn sidecar_path(settings: &Settings) -> PathBuf {
    let path = settings.threads_db_path();
    let legacy = settings.data_dir.join(LEGACY_SIDECAR);
    if !path.exists() && legacy.exists() {
        legacy
    } else {
        path
    }
}

/// Rename a legacy `ai_session_sidecar.db` to `threads.db`, for the daemon to call before it
/// opens the sidecar. The WAL and shared-memory files move first and the database last, so a
/// crash midway leaves the legacy database to finish moving on the next start.
pub fn migrate_sidecar_name(settings: &Settings) -> std::io::Result<()> {
    let path = settings.threads_db_path();
    let legacy = settings.data_dir.join(LEGACY_SIDECAR);
    if path.exists() || !legacy.exists() {
        return Ok(());
    }
    for suffix in ["-wal", "-shm"] {
        let from = PathBuf::from(format!("{}{suffix}", legacy.display()));
        if from.exists() {
            std::fs::rename(&from, format!("{}{suffix}", path.display()))?;
        }
    }
    std::fs::rename(legacy, path)
}

/// Have the daemon reproject the sidecar from the whole record store on its next start, for
/// commands that re-encrypt the records under it (which changes none of what they say).
pub async fn invalidate_sidecar(settings: &Settings) -> Result<(), DbError> {
    AiSessionDatabase::invalidate_projection(sidecar_path(settings)).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[rstest::rstest]
    fn legacy_sidecar_moves_to_threads_db() {
        let dir = tempfile::tempdir().unwrap();
        let mut settings = Settings::utc();
        settings.data_dir = dir.path().to_path_buf();
        for name in [LEGACY_SIDECAR, "ai_session_sidecar.db-wal", "ai_session_sidecar.db-shm"] {
            std::fs::write(dir.path().join(name), name).unwrap();
        }
        assert_eq!(sidecar_path(&settings), dir.path().join(LEGACY_SIDECAR));

        migrate_sidecar_name(&settings).unwrap();

        assert_eq!(sidecar_path(&settings), dir.path().join("threads.db"));
        for suffix in ["", "-wal", "-shm"] {
            let legacy = format!("{LEGACY_SIDECAR}{suffix}");
            assert!(!dir.path().join(&legacy).exists());
            let moved = dir.path().join(format!("threads.db{suffix}"));
            assert_eq!(std::fs::read_to_string(moved).unwrap(), legacy);
        }
    }
}
