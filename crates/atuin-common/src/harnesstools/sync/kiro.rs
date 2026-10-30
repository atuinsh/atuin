//! Kiro: the session directory `<sessions>/<bucket>/<session>/`, whose log atuin reads but
//! never writes (see the `kiro` module), so a copy here is neither caught up nor switched.
//!
//! **Liveness.** Kiro keeps no lock on a session it has open. Its header's `status` is `idle`
//! between turns (and `completed` or `failed` once a session is over) and `in_progress` or
//! `waiting_on_user` while a turn runs: a running turn whose header or log changed in the last
//! [`RECENT`](super::RECENT) says a Kiro has the session, wherever it runs (`kiro-cli chat
//! --resume-id` reopens a session from anywhere). A Kiro that died mid-turn leaves `in_progress`
//! behind for good, so a stale one says only that someone may. Kiro rewrites the header in place,
//! so one that doesn't parse is being written. Between turns, a log still changing may have a
//! Kiro at its prompt.

use std::path::Path;

use super::liveness::changed_lately;
use super::{
    AppendOptions, AppendOutcome, Liveness, LocalTip, RehydrateMessage, ReplaceOutcome,
    SessionSync, SyncError, blocking_liveness, modified,
};
use crate::harnesstools::kiro::Kiro;
use crate::harnesstools::kiro::session::{HEADER_FILE, LOG_FILE, default_root, session_dir};

const UNSUPPORTED: SyncError = SyncError::Unsupported("Kiro sessions can't be written here");

impl SessionSync for Kiro {
    async fn local_tip(&self, _id: &str) -> Result<Option<LocalTip>, SyncError> {
        Err(UNSUPPORTED)
    }

    async fn is_live(&self, id: &str, _cwd: Option<&Path>) -> Liveness {
        let root = default_root();
        let id = id.to_owned();
        blocking_liveness(move || match session_dir(&root, &id) {
            Some(dir) => liveness(&dir),
            None => Liveness::NotLive,
        })
        .await
    }

    async fn append(
        &self,
        _id: &str,
        _base: &LocalTip,
        _lines: &[RehydrateMessage],
        _options: &AppendOptions<'_>,
    ) -> Result<AppendOutcome, SyncError> {
        Err(UNSUPPORTED)
    }

    async fn replace(
        &self,
        _id: &str,
        _base: &LocalTip,
        _branch: &[RehydrateMessage],
        _options: &AppendOptions<'_>,
        _backups: &Path,
    ) -> Result<ReplaceOutcome, SyncError> {
        Err(UNSUPPORTED)
    }
}

/// What the header's `status` says of the session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Status {
    /// A turn is running.
    Busy,
    /// Between turns, or over.
    Idle,
    /// No header.
    Missing,
    /// A header that can't be read, or a status this reader doesn't know.
    Unreadable,
}

fn status(header: &Path) -> Status {
    let text = match std::fs::read_to_string(header) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Status::Missing,
        Err(_) => return Status::Unreadable,
    };
    let Ok(header) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Status::Unreadable;
    };
    match header["status"].as_str() {
        Some("in_progress" | "waiting_on_user") => Status::Busy,
        Some("idle" | "completed" | "failed") | None => Status::Idle,
        Some(_) => Status::Unreadable,
    }
}

/// Whether a Kiro has the session in `dir` open (see the module docs).
fn liveness(dir: &Path) -> Liveness {
    let header = dir.join(HEADER_FILE);
    let log_changing = || changed_lately(modified(&dir.join(LOG_FILE)));
    match status(&header) {
        // A turn's progress rewrites the header and appends to the log.
        Status::Busy if log_changing() || changed_lately(modified(&header)) => {
            Liveness::Live { pid: None }
        }
        Status::Busy | Status::Unreadable => Liveness::Unknown,
        Status::Idle | Status::Missing if log_changing() => Liveness::Unknown,
        Status::Idle | Status::Missing => Liveness::NotLive,
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    const HEADER: &str = include_str!("../../../tests/fixtures/kiro/probe.session.json");

    /// A session directory whose log last changed an hour ago, with `header` beside it, written
    /// just now.
    fn session(header: Option<&str>) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join(LOG_FILE);
        std::fs::write(&log, "{}\n").unwrap();
        age(&log);
        if let Some(header) = header {
            std::fs::write(dir.path().join(HEADER_FILE), header).unwrap();
        }
        dir
    }

    /// Make `path` an hour old.
    fn age(path: &Path) {
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
        std::fs::File::options().write(true).open(path).unwrap().set_modified(old).unwrap();
    }

    fn with_status(status: &str) -> String {
        HEADER.replace(r#""status": "idle""#, &format!(r#""status": "{status}""#))
    }

    /// The header was just rewritten with the status, as a turn's progress rewrites it.
    #[rstest]
    #[case::running("in_progress", Liveness::Live { pid: None })]
    #[case::asking("waiting_on_user", Liveness::Live { pid: None })]
    #[case::idle("idle", Liveness::NotLive)]
    #[case::over("completed", Liveness::NotLive)]
    #[case::failed("failed", Liveness::NotLive)]
    #[case::unknown_status("paused", Liveness::Unknown)]
    fn the_headers_status_says_whether_a_turn_runs(#[case] status: &str, #[case] want: Liveness) {
        assert!(HEADER.contains(r#""status": "idle""#));
        let dir = session(Some(&with_status(status)));
        assert_eq!(liveness(dir.path()), want);
    }

    /// A Kiro that died mid-turn leaves `in_progress` behind: with nothing written for a while,
    /// it says only that someone may have the session.
    #[rstest]
    #[case::running("in_progress")]
    #[case::asking("waiting_on_user")]
    fn a_stale_running_turn_may_or_may_not_have_a_writer(#[case] status: &str) {
        let dir = session(Some(&with_status(status)));
        age(&dir.path().join(HEADER_FILE));
        assert_eq!(liveness(dir.path()), Liveness::Unknown);
        // The log still changing says the turn goes on.
        std::fs::write(dir.path().join(LOG_FILE), "{}\n{}\n").unwrap();
        assert_eq!(liveness(dir.path()), Liveness::Live { pid: None });
    }

    /// Kiro rewrites the header in place: one that doesn't parse is being written.
    #[rstest]
    fn a_half_written_header_is_a_writer() {
        let dir = session(Some(&HEADER[..HEADER.len() / 2]));
        assert_eq!(liveness(dir.path()), Liveness::Unknown);
    }

    /// Between turns, the log says: untouched for a while, no one has it; just written, someone
    /// may.
    #[rstest]
    fn between_turns_a_changing_log_may_have_a_writer() {
        let dir = session(None);
        assert_eq!(liveness(dir.path()), Liveness::NotLive);
        std::fs::write(dir.path().join(LOG_FILE), "{}\n{}\n").unwrap();
        assert_eq!(liveness(dir.path()), Liveness::Unknown);
        let dir = session(Some(HEADER));
        std::fs::write(dir.path().join(LOG_FILE), "{}\n{}\n").unwrap();
        assert_eq!(liveness(dir.path()), Liveness::Unknown);
    }
}
