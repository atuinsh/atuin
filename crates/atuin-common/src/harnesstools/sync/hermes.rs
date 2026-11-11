//! Hermes Agent: the session's rows in a `state.db`, which atuin reads but never writes (see the
//! `hermes` module), so a copy here is neither caught up nor switched.
//!
//! **Liveness.** A Hermes chat client (CLI, TUI) with a session open, at its prompt or not,
//! claims it in its home's registry, `runtime/active_sessions.json`: an entry naming the session
//! whose process is running (and, where its start time can be read, started when the entry says)
//! has it open ([`Liveness::Live`]). Nothing in `state.db` says so between turns. A turn holds a
//! lease on its conversation in `session_turn_leases` for as long as it runs, refreshed as it goes
//! and deleted at its end: an unexpired one says a Hermes has the session now, whatever surface
//! runs it. The lease is on the compression lineage's root, the session
//! itself unless it carries on one Hermes ended for compression. Between turns nothing marks the
//! session held, and `ended_at` is cleared again by a resume and set late by the reaper of a
//! gateway that died: a session not ended that Hermes touched in the last
//! [`RECENT`](super::RECENT) may have a Hermes at its prompt ([`Liveness::Unknown`]). A store
//! that can't be read says the same.

use std::path::Path;
use std::time::SystemTime;

use super::liveness::{Processes, Start};
use super::{
    AppendOptions, AppendOutcome, Liveness, LocalTip, RehydrateMessage, ReplaceOutcome,
    SessionSync, SyncError, blocking_liveness,
};
use crate::harnesstools::hermes::store::{SessionRow, Store};
use crate::harnesstools::hermes::{Hermes, hermes_home, session};
use crate::path::PathBufExt as _;

const UNSUPPORTED: SyncError = SyncError::Unsupported("Hermes sessions can't be written here");

impl SessionSync for Hermes {
    async fn local_tip(&self, _id: &str) -> Result<Option<LocalTip>, SyncError> {
        Err(UNSUPPORTED)
    }

    async fn is_live(&self, id: &str, _cwd: Option<&Path>) -> Liveness {
        let Some(db) = session::locate(&hermes_home(), id).await else {
            return Liveness::NotLive;
        };
        let registry = db.with_file_name("runtime").joined(REGISTRY_FILE);
        let of = id.to_owned();
        let claimed =
            blocking_liveness(move || registry_holder(&registry, &of, &Processes::here())).await;
        match claimed {
            Liveness::Live { .. } => claimed,
            Liveness::NotLive => liveness_in(&db, id, SystemTime::now()).await,
            // A claim that can't be read may be the client at its prompt; a lease would say the
            // same as a running turn, and nothing else says more.
            Liveness::Unknown => match liveness_in(&db, id, SystemTime::now()).await {
                live @ Liveness::Live { .. } => live,
                _ => Liveness::Unknown,
            },
        }
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

/// Whether a Hermes has session `id` of the store `db` open at `now` (see the module docs).
pub(super) async fn liveness_in(db: &Path, id: &str, now: SystemTime) -> Liveness {
    let Ok(mut store) = Store::open(db).await else {
        return Liveness::Unknown;
    };
    let read = async {
        let Some(session) = store.any_session(id).await? else {
            return Ok(None);
        };
        let root = store.lineage_root(id).await?;
        let secs = now.duration_since(SystemTime::UNIX_EPOCH).map_or(0.0, |at| at.as_secs_f64());
        let leased = store.leased(&root, secs).await?;
        Ok::<_, sqlx::Error>(Some((session, leased)))
    }
    .await;
    store.close().await;
    match read {
        Ok(Some((_, true))) => Liveness::Live { pid: None },
        Ok(Some((session, false))) => idle(&session, now),
        Ok(None) => Liveness::NotLive,
        Err(_) => Liveness::Unknown,
    }
}

/// The registry under a Hermes home's `runtime/` where chat clients claim the sessions they have
/// open (`hermes_cli/active_sessions.py`).
const REGISTRY_FILE: &str = "active_sessions.json";

/// Whether a Hermes chat client claimed session `id` in `registry` and is running: `{"entries":
/// [...]}` (or a bare list), each naming its session (`session_id`, or for a client that moved on
/// to a new one, `metadata.live_session_id`), its `pid` and that process's `process_start_time`
/// (psutil's, epoch seconds). An entry whose process is gone, or another started under its pid,
/// is one Hermes would prune as stale.
fn registry_holder(registry: &Path, id: &str, procs: &Processes) -> Liveness {
    let bytes = match std::fs::read(registry) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Liveness::NotLive,
        Err(_) => return Liveness::Unknown,
    };
    // Hermes replaces the file whole, so a torn read is no state it writes.
    let Ok(data) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return Liveness::Unknown;
    };
    let entries = data.get("entries").unwrap_or(&data);
    let Some(entries) = entries.as_array() else {
        return Liveness::Unknown;
    };
    let mut unknown = false;
    for entry in entries {
        let names = |key: &str| entry[key].as_str() == Some(id);
        if !(names("session_id") || entry["metadata"]["live_session_id"].as_str() == Some(id)) {
            continue;
        }
        let Some(pid) = entry["pid"].as_u64().and_then(|pid| u32::try_from(pid).ok()) else {
            unknown = true;
            continue;
        };
        match procs.running(pid) {
            Ok(Some(running)) => {
                let claimed = entry["process_start_time"].as_f64();
                let reused = match (running.start, claimed) {
                    (Some(Start::Epoch(started)), Some(claimed)) => {
                        // `sysinfo` reads whole seconds; psutil's are fractional.
                        #[allow(clippy::cast_precision_loss)]
                        let started = started as f64;
                        (started - claimed).abs() > 1.0
                    }
                    // Boot ticks (Linux) can't be set against epoch seconds: a running pid
                    // counts, as Hermes's own lenient check counts it.
                    _ => false,
                };
                if !reused {
                    return Liveness::Live { pid: Some(pid) };
                }
            }
            Ok(None) => {}
            Err(_) => unknown = true,
        }
    }
    if unknown {
        Liveness::Unknown
    } else {
        Liveness::NotLive
    }
}

/// [`liveness_in`] for a session no turn holds.
fn idle(session: &SessionRow, now: SystemTime) -> Liveness {
    if session.ended_at.is_some() {
        return Liveness::NotLive;
    }
    let touched = SystemTime::UNIX_EPOCH
        .checked_add(std::time::Duration::try_from_secs_f64(session.touched()).unwrap_or_default());
    // A time later than `now` (clocks disagree) counts as recent.
    match touched.and_then(|at| now.duration_since(at).ok()) {
        Some(ago) if ago >= super::RECENT => Liveness::NotLive,
        _ => Liveness::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use rstest::rstest;

    use super::*;
    use crate::harnesstools::hermes::session::tests::{execute_in, probe_store};
    use crate::harnesstools::sync::liveness::tests::{entry, table};

    /// The client the registry cases below run: pid 4934, started at 1794263000 (whole seconds,
    /// as `sysinfo` reads it).
    fn client() -> Processes {
        table(&[entry(4934, "python3.14", &["python3", "-m", "hermes_cli.main"], 1_794_263_000)])
    }

    /// An idle client at its prompt holds nothing in `state.db`; its claim in the registry says
    /// it has the session open, as long as its process is the one that claimed it.
    #[rstest]
    #[case::claimed(r#"{"entries":[{"session_id":"s1","pid":4934,"process_start_time":1794263000.42}]}"#, Liveness::Live { pid: Some(4934) })]
    #[case::bare_list(r#"[{"session_id":"s1","pid":4934,"process_start_time":1794263000.42}]"#, Liveness::Live { pid: Some(4934) })]
    #[case::moved_on(r#"{"entries":[{"session_id":"s0","metadata":{"live_session_id":"s1"},"pid":4934}]}"#, Liveness::Live { pid: Some(4934) })]
    #[case::another_session(r#"{"entries":[{"session_id":"s2","pid":4934}]}"#, Liveness::NotLive)]
    #[case::process_gone(
        r#"{"entries":[{"session_id":"s1","pid":5150,"process_start_time":1794263000.0}]}"#,
        Liveness::NotLive
    )]
    #[case::pid_reused(
        r#"{"entries":[{"session_id":"s1","pid":4934,"process_start_time":1794260000.0}]}"#,
        Liveness::NotLive
    )]
    #[case::no_pid(r#"{"entries":[{"session_id":"s1"}]}"#, Liveness::Unknown)]
    #[case::unreadable("{\"entries\": [", Liveness::Unknown)]
    fn a_client_claim_in_the_registry_is_live(#[case] registry: &str, #[case] want: Liveness) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(REGISTRY_FILE);
        std::fs::write(&path, registry).unwrap();
        assert_eq!(registry_holder(&path, "s1", &client()), want);
    }

    #[rstest]
    fn no_registry_is_no_claim() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            registry_holder(&dir.path().join(REGISTRY_FILE), "s1", &client()),
            Liveness::NotLive
        );
    }

    /// The probe's session whose turn the provider refused.
    const FAILED: &str = "local-86e8f7d82d63ddc82b3bed02f8e342917838bccabbcc3d3ca79ce9d35a2c1d62";
    /// The probe's resumed session: started at 1794246206.6 (unix seconds), ended at 1794246525.
    const RESUMED: &str = "local-8baebc867cd817b810a10420638858ba79ef3e78ad6ada513ccf9d79bd31d565";

    fn at(secs: f64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::try_from_secs_f64(secs).unwrap()
    }

    #[rstest]
    #[case::ended(None, None, Liveness::NotLive)]
    #[case::leased(None, Some(1_794_246_800.0), Liveness::Live { pid: None })]
    #[case::lease_expired(None, Some(1_794_246_500.0), Liveness::NotLive)]
    #[case::open_and_recent(Some(1_794_246_590.0), None, Liveness::Unknown)]
    #[case::open_and_idle(Some(1_794_246_000.0), None, Liveness::NotLive)]
    #[tokio::test]
    async fn a_session_is_live_by_its_lease_then_its_activity(
        #[case] open_since: Option<f64>,
        #[case] lease_until: Option<f64>,
        #[case] want: Liveness,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let db = probe_store(dir.path()).await;
        let mut sql = String::new();
        if let Some(touched) = open_since {
            sql.push_str(&format!(
                "UPDATE sessions SET ended_at = NULL, end_reason = NULL, last_activity_at = \
                 {touched}
                     WHERE id = '{RESUMED}';
                 DELETE FROM messages WHERE session_id = '{RESUMED}' AND timestamp > {touched};"
            ));
        }
        if let Some(until) = lease_until {
            sql.push_str(&format!(
                "INSERT INTO session_turn_leases (conversation_id, holder, acquired_at, \
                 expires_at)
                     VALUES ('{RESUMED}', 'pid=1:turn=t:platform=cli', 1794246300.0, {until});"
            ));
        }
        execute_in(&db, &sql).await;
        assert_eq!(liveness_in(&db, RESUMED, at(1_794_246_600.0)).await, want);
    }

    /// A turn's lease is on the compression lineage's root, so a rotated session is live by its
    /// parent's lease.
    #[rstest]
    #[tokio::test]
    async fn a_rotated_session_is_live_by_its_roots_lease() {
        let dir = tempfile::tempdir().unwrap();
        let db = probe_store(dir.path()).await;
        execute_in(
            &db,
            &format!(
                "UPDATE sessions SET end_reason = 'compression' WHERE id = '{FAILED}';
                 UPDATE sessions SET parent_session_id = '{FAILED}' WHERE id = '{RESUMED}';
                 INSERT INTO session_turn_leases (conversation_id, holder, acquired_at, expires_at)
                     VALUES ('{FAILED}', 'pid=1:turn=t:platform=cli', 1794246300.0, 1794246800.0);"
            ),
        )
        .await;
        assert_eq!(liveness_in(&db, RESUMED, at(1_794_246_600.0)).await, Liveness::Live {
            pid: None
        });
    }

    #[rstest]
    #[tokio::test]
    async fn a_session_the_store_lacks_is_not_live() {
        let dir = tempfile::tempdir().unwrap();
        let db = probe_store(dir.path()).await;
        assert_eq!(liveness_in(&db, "no-such", at(1_794_246_600.0)).await, Liveness::NotLive);
    }
}
