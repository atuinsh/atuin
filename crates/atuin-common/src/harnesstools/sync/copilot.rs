//! Copilot: the session directory `<sessions>/<session>/`, whose log atuin reads but never
//! writes (see the `copilot` module), so a copy here is neither caught up nor switched.
//!
//! **Liveness.** A Copilot process that has a session open keeps `inuse.<pid>.lock` in the
//! session's directory, with its pid as the content, and removes it on exit: a lock whose
//! process runs says the session is open, wherever the process works (`copilot --resume` reopens
//! a session from anywhere). A lock a crash left behind names a process that is gone. Without a
//! live lock, a log still changing in the last [`RECENT`](super::RECENT) may have a writer whose
//! lock can't be read.

use std::path::Path;

use super::liveness::changed_lately;
use super::{
    AppendOptions, AppendOutcome, Liveness, LocalTip, Processes, RehydrateMessage, ReplaceOutcome,
    SessionSync, SyncError, blocking_liveness, modified,
};
use crate::harnesstools::copilot::Copilot;
use crate::harnesstools::copilot::session::{EVENTS_FILE, default_root, session_dir};

const UNSUPPORTED: SyncError = SyncError::Unsupported("Copilot sessions can't be written here");

impl SessionSync for Copilot {
    async fn local_tip(&self, _id: &str) -> Result<Option<LocalTip>, SyncError> {
        Err(UNSUPPORTED)
    }

    async fn is_live(&self, id: &str, _cwd: Option<&Path>) -> Liveness {
        let Some(dir) = session_dir(&default_root(), id) else {
            return Liveness::NotLive;
        };
        blocking_liveness(move || liveness(&dir, &Processes::here())).await
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

/// Whether a Copilot process has the session in `dir` open (see the module docs).
fn liveness(dir: &Path, procs: &Processes) -> Liveness {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Liveness::NotLive;
    };
    let mut unreadable = false;
    for pid in entries.filter_map(Result::ok).filter_map(|entry| lock_pid(&entry.path())) {
        match procs.running(pid) {
            Ok(Some(_)) => return Liveness::Live { pid: Some(pid) },
            Ok(None) => {}
            Err(_) => unreadable = true,
        }
    }
    if unreadable || changed_lately(modified(&dir.join(EVENTS_FILE))) {
        Liveness::Unknown
    } else {
        Liveness::NotLive
    }
}

/// The pid an `inuse.<pid>.lock` file names, from its name.
fn lock_pid(path: &Path) -> Option<u32> {
    let name = path.file_name()?.to_str()?;
    name.strip_prefix("inuse.")?.strip_suffix(".lock")?.parse().ok()
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case::lock("inuse.22629.lock", Some(22629))]
    #[case::hold("inuse.22629.hold", None)]
    #[case::other("events.jsonl", None)]
    #[case::not_a_pid("inuse.x.lock", None)]
    fn lock_files_name_their_pid(#[case] name: &str, #[case] want: Option<u32>) {
        assert_eq!(lock_pid(&Path::new("/s").join(name)), want);
    }

    #[rstest]
    fn a_session_with_no_lock_and_a_stale_log_is_not_live() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(EVENTS_FILE), "{}\n").unwrap();
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
        std::fs::File::options()
            .write(true)
            .open(dir.path().join(EVENTS_FILE))
            .unwrap()
            .set_modified(old)
            .unwrap();
        assert_eq!(liveness(dir.path(), &Processes::here()), Liveness::NotLive);
    }

    /// A lock whose process runs says the session is open; one a crash left behind says
    /// nothing, and the log, untouched for a while, says no one has it.
    #[rstest]
    fn a_lock_names_the_process_that_has_the_session_open() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(EVENTS_FILE), "{}\n").unwrap();
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
        std::fs::File::options()
            .write(true)
            .open(dir.path().join(EVENTS_FILE))
            .unwrap()
            .set_modified(old)
            .unwrap();
        std::fs::write(dir.path().join("inuse.4242.lock"), "4242").unwrap();
        let table = |pids: &[u32]| {
            Processes::Table(
                pids.iter()
                    .map(|&pid| super::super::TableEntry {
                        pid,
                        name: "copilot".into(),
                        cmd: vec!["copilot".into()],
                        start_time: 1,
                        zombie: false,
                        cwd: None,
                    })
                    .collect(),
            )
        };
        assert_eq!(liveness(dir.path(), &table(&[4242])), Liveness::Live { pid: Some(4242) });
        assert_eq!(liveness(dir.path(), &table(&[1])), Liveness::NotLive, "a stale lock");
        assert_eq!(liveness(dir.path(), &Processes::Unavailable), Liveness::Unknown);
    }
}
