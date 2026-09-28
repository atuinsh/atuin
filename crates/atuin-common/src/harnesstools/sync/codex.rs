//! Codex: liveness only, for now. Catching a rollout up (a segment file of its own) comes later.
//!
//! **Liveness.** A Codex process holds an exclusive `flock` on
//! `$CODEX_HOME/thread-writer-locks/<thread>.lock` for as long as it has the thread loaded, and
//! removes the file when it lets go (codex-rs `rollout/src/writer_lock.rs`; seen held by `codex
//! app-server` 0.157 with a thread loaded, and gone after it exited). A Codex old enough to keep
//! no locks has no such directory; its processes are looked for instead.

use std::path::{Path, PathBuf};

use super::liveness::{Claim, ProcessInfo, flock_holder, scan};
use super::{AppendOptions, AppendOutcome, Liveness, LocalTip, Processes, SessionSync, SyncError};
use crate::harnesstools::codex::Codex;
use crate::harnesstools::codex::session::resume_id;
use crate::harnesstools::rehydrate::RehydrateMessage;
use crate::utils::{env_nonempty, home_dir};

impl SessionSync for Codex {
    async fn local_tip(&self, _id: &str) -> Result<Option<LocalTip>, SyncError> {
        Err(SyncError::Unsupported("Codex sessions can't be caught up yet"))
    }

    async fn is_live(&self, id: &str, cwd: Option<&Path>) -> Liveness {
        let (home, id, cwd) = (codex_home(), id.to_owned(), cwd.map(Path::to_path_buf));
        tokio::task::spawn_blocking(move || {
            liveness(&home, &id, cwd.as_deref(), &Processes::here())
        })
        .await
        .unwrap_or(Liveness::Unknown)
    }

    async fn append(
        &self,
        _id: &str,
        _base: &LocalTip,
        _lines: &[RehydrateMessage],
        _options: &AppendOptions<'_>,
    ) -> Result<AppendOutcome, SyncError> {
        Err(SyncError::Unsupported("Codex sessions can't be caught up yet"))
    }
}

/// `$CODEX_HOME`, else `~/.codex`.
fn codex_home() -> PathBuf {
    env_nonempty("CODEX_HOME").map_or_else(|| home_dir().join(".codex"), PathBuf::from)
}

/// Whether a Codex process has session `id` (a thread, or a segment of one) loaded.
pub(super) fn liveness(home: &Path, id: &str, cwd: Option<&Path>, procs: &Processes) -> Liveness {
    let thread = resume_id(id);
    if !crate::harnesstools::resume::is_plain_name(thread) {
        return Liveness::NotLive;
    }
    let locks = home.join("thread-writer-locks");
    if locks.is_dir() {
        return flock_holder(&locks.join(format!("{thread}.lock")), procs);
    }
    match procs.list() {
        Ok(processes) => scan(&processes, cwd, |p| claim(p, thread)),
        Err(_) => Liveness::Unknown,
    }
}

/// What a Codex process (the native binary, or the node launcher in front of it) claims of
/// thread `thread`.
fn claim(process: &ProcessInfo, thread: &str) -> Claim {
    let name = |arg: &str| {
        let name = arg.rsplit(['/', '\\']).next().unwrap_or(arg);
        matches!(name, "codex" | "codex.exe" | "codex.js")
    };
    let args = match process.argv.as_slice() {
        [program, rest @ ..] if name(program) => rest,
        [_, script, rest @ ..] if name(script) => rest,
        _ => return Claim::None,
    };
    let words: Vec<&str> =
        args.iter().map(String::as_str).filter(|a| !a.starts_with('-')).collect();
    match words.as_slice() {
        ["resume", session, ..] | ["exec", "resume", session, ..] => {
            Claim::Session(*session == thread)
        }
        ["fork" | "login" | "logout" | "completion" | "help" | "apply" | "debug", ..] => {
            Claim::None
        }
        ["app-server" | "mcp-server" | "proto" | "cloud", ..] => Claim::Any,
        _ => Claim::Directory(None),
    }
}

#[cfg(test)]
mod tests;
