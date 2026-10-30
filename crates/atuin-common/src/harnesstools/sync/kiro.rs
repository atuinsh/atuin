//! Kiro: the session directory `<sessions>/<bucket>/<session>/`, whose log atuin reads but
//! never writes (see the `kiro` module), so a copy here is neither caught up nor switched.
//!
//! **Liveness.** Kiro keeps no lock on a session it has open. Its header's `status` is `idle`
//! between turns (and `completed` or `failed` once a session is over) and `in_progress` or
//! `waiting_on_user` while a turn runs: a running turn says a Kiro has the session, wherever it
//! runs (`kiro-cli chat --resume-id` reopens a session from anywhere). Kiro rewrites the header
//! in place, so one that doesn't parse is being written. Between turns, a log still changing in
//! the last [`RECENT`](super::RECENT) may have a Kiro at its prompt.

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
    match status(&dir.join(HEADER_FILE)) {
        Status::Busy => Liveness::Live { pid: None },
        Status::Unreadable => Liveness::Unknown,
        Status::Idle | Status::Missing if changed_lately(modified(&dir.join(LOG_FILE))) => {
            Liveness::Unknown
        }
        Status::Idle | Status::Missing => Liveness::NotLive,
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    const HEADER: &str = include_str!("../../../tests/fixtures/kiro/probe.session.json");

    /// A session directory whose log last changed an hour ago, with `header` beside it.
    fn session(header: Option<&str>) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join(LOG_FILE);
        std::fs::write(&log, "{}\n").unwrap();
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
        std::fs::File::options().write(true).open(&log).unwrap().set_modified(old).unwrap();
        if let Some(header) = header {
            std::fs::write(dir.path().join(HEADER_FILE), header).unwrap();
        }
        dir
    }

    fn with_status(status: &str) -> String {
        HEADER.replace(r#""status": "idle""#, &format!(r#""status": "{status}""#))
    }

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
