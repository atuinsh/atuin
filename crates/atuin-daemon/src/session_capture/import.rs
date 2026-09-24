use std::sync::Arc;

use atuin_client::ai_session::{Appended, HarnessKind, NativeSessionId};
use atuin_common::harnesstools::AnyHarness;
use atuin_common::harnesstools::session::any::AnySessions;
use atuin_common::sync::BlockingPool;
use futures::{Stream, StreamExt};

use super::Sink;
use super::message_enricher::MessageEnricher;

#[derive(Debug, Clone)]
pub enum ImportProgress {
    Session {
        harness: HarnessKind,
        session: NativeSessionId,
        imported: u64,
        skipped: u64,
        failed: u64,
    },
    Finished {
        sessions: u64,
        imported: u64,
        skipped: u64,
        failed: u64,
    },
    /// A directory, entry, or file-type read failed mid-scan, so the backfill is partial. Folded
    /// into the summary's `failed` by `SessionImporter::run`; never surfaced as its own event.
    ScanFailed {
        failed: u64,
    },
}

pub struct SessionImporter {
    sink: Arc<Sink>,
    pool: BlockingPool,
    concurrency: usize,
}

impl SessionImporter {
    pub fn new(sink: Arc<Sink>, pool: BlockingPool) -> Self {
        Self {
            sink,
            pool,
            concurrency: 8,
        }
    }

    pub fn run(
        self,
        filter: Option<HarnessKind>,
    ) -> impl Stream<Item = ImportProgress> + Send + 'static {
        async_stream::stream! {
            let mut sessions = 0u64;
            let mut imported = 0u64;
            let mut skipped = 0u64;
            let mut failed = 0u64;
            for harness in AnyHarness::all() {
                let kind = HarnessKind::from(harness);
                if filter.is_some_and(|want| want != kind) {
                    continue;
                }
                let Some(observed) = harness.sessions(&self.pool) else {
                    continue;
                };
                let progress = self.harness(kind, observed);
                futures::pin_mut!(progress);
                while let Some(event) = progress.next().await {
                    let mut passthrough = true;
                    match &event {
                        ImportProgress::Session { imported: i, skipped: s, failed: f, .. } => {
                            sessions += 1;
                            imported += i;
                            skipped += s;
                            failed += f;
                        }
                        // A scan failure is not a session: it lifts the summary's `failed` but is
                        // not streamed as its own progress event.
                        ImportProgress::ScanFailed { failed: f } => {
                            failed += f;
                            passthrough = false;
                        }
                        ImportProgress::Finished { .. } => {}
                    }
                    if passthrough {
                        yield event;
                    }
                }
            }
            yield ImportProgress::Finished { sessions, imported, skipped, failed };
        }
    }

    pub fn harness(
        &self,
        kind: HarnessKind,
        sessions: AnySessions,
    ) -> impl Stream<Item = ImportProgress> + Send + 'static {
        let sink = self.sink.clone();
        let concurrency = self.concurrency;
        async_stream::stream! {
            let Ok(existing) = sessions.existing() else {
                return;
            };
            let mut imports = existing
                .map(|item| {
                    let sink = sink.clone();
                    async move {
                        // A scan failure (unreadable directory/entry) is not a session; surface it
                        // so the summary reflects a partial backfill instead of a silent one.
                        let Ok(session) = item else {
                            return ImportProgress::ScanFailed { failed: 1 };
                        };
                        // One enricher per session: it carries that session's bookkeeping (title,
                        // timestamps, parent, synthetic ids) across its lines, exactly as live
                        // capture does, so a backfilled row matches the captured one.
                        let mut enricher = MessageEnricher::new(kind);
                        let sid = session.id();
                        let mut imported = 0u64;
                        let mut skipped = 0u64;
                        let mut failed = 0u64;
                        let mut messages = session.read();
                        let mut done = false;
                        while !done {
                            let rows = match messages.next().await {
                                Some(Ok(message)) => enricher.capture(&sid, &message),
                                Some(Err(_)) => {
                                    failed += 1;
                                    continue;
                                }
                                None => {
                                    done = true;
                                    enricher.finish(&sid)
                                }
                            };
                            // A bookkeeping line worth no row (matches live capture) is not
                            // counted: it is neither a new record nor a dedupe skip.
                            for msg in rows {
                                match sink.append(msg).await {
                                    Ok(Appended::New) => imported += 1,
                                    Ok(Appended::Duplicate) => skipped += 1,
                                    Err(_) => failed += 1,
                                }
                            }
                        }
                        ImportProgress::Session {
                            harness: kind,
                            session: enricher.handle(&sid).session,
                            imported,
                            skipped,
                            failed,
                        }
                    }
                })
                .buffer_unordered(concurrency);
            while let Some(event) = imports.next().await {
                yield event;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use atuin_client::ai_session::{
        AiSessionDatabase, AiSessionStore, HarnessSession, NativeSessionId,
    };
    use atuin_client::record::sqlite_store::SqliteStore;
    use atuin_common::encryption::paseto_v4::Key;
    use atuin_common::harnesstools::pi::session::PiSessions;
    use atuin_domain::record::HostId;
    use futures::StreamExt;
    use rstest::rstest;

    use super::*;
    use crate::session_capture::AiHarnessSessionCapture;

    async fn mem_store() -> AiSessionStore {
        let store = SqliteStore::in_memory(std::time::Duration::from_secs(5)).await.unwrap();
        AiSessionStore::builder()
            .store(store)
            .host_id(HostId(atuin_common::utils::uuid_v7()))
            .key(Key::generate())
            .build()
    }

    /// A pi session file: the `session` header pi starts every session with, then `turns`.
    fn write_pi_session(root: &Path, id: &str, turns: &[&str]) {
        let header = serde_json::json!({"type": "session", "version": 3, "id": id}).to_string();
        let body = std::iter::once(header)
            .chain(turns.iter().enumerate().map(|(i, turn)| {
                let (role, text) = turn.split_once(':').unwrap();
                serde_json::json!({
                    "type": "message",
                    "id": format!("{id}-m{i}"),
                    "message": {"role": role, "content": text},
                })
                .to_string()
            }))
            .collect::<Vec<_>>()
            .join("\n")
            // Trailing newline: read() reads complete lines only (like live capture), so a real
            // session terminates its final record.
            + "\n";
        std::fs::write(root.join(format!("1700000000_{id}.jsonl")), body).unwrap();
    }

    fn pool() -> BlockingPool {
        BlockingPool::new(std::num::NonZeroUsize::MIN)
    }

    fn pi_sessions(root: &Path) -> AnySessions {
        AnySessions::from(PiSessions::builder().root(root.to_path_buf()).pool(pool()).build())
    }

    fn sum_new(progress: &[ImportProgress]) -> u64 {
        progress
            .iter()
            .map(|event| match event {
                ImportProgress::Session { imported, .. } => *imported,
                ImportProgress::Finished { .. } | ImportProgress::ScanFailed { .. } => 0,
            })
            .sum()
    }

    fn sum_skipped(progress: &[ImportProgress]) -> u64 {
        progress
            .iter()
            .map(|event| match event {
                ImportProgress::Session { skipped, .. } => *skipped,
                ImportProgress::Finished { .. } | ImportProgress::ScanFailed { .. } => 0,
            })
            .sum()
    }

    #[rstest]
    #[tokio::test]
    async fn import_is_idempotent_and_accounts_new_vs_skipped() {
        let root = tempfile::tempdir().unwrap();
        write_pi_session(root.path(), "s1", &["user:hi", "assistant:yo"]);

        let sink =
            Arc::new(Sink::new(mem_store().await, AiSessionDatabase::in_memory().await.unwrap()));
        let handle = HarnessSession {
            harness: HarnessKind::Pi,
            session: NativeSessionId::from("s1".to_owned()),
        };

        let first: Vec<_> = SessionImporter::new(sink.clone(), pool())
            .harness(HarnessKind::Pi, pi_sessions(root.path()))
            .collect()
            .await;
        // The header and the two turns; the session counts the turns.
        assert_eq!(sum_new(&first), 3);
        assert_eq!(sum_skipped(&first), 0);
        let after_first = sink.sidecar.get_session(&handle).await.unwrap().unwrap().message_count;
        assert_eq!(after_first, 2);

        let second: Vec<_> = SessionImporter::new(sink.clone(), pool())
            .harness(HarnessKind::Pi, pi_sessions(root.path()))
            .collect()
            .await;
        assert_eq!(sum_new(&second), 0);
        assert_eq!(sum_skipped(&second), 3);
        let after_second = sink.sidecar.get_session(&handle).await.unwrap().unwrap().message_count;
        assert_eq!(after_second, after_first);
    }

    #[rstest]
    #[tokio::test]
    async fn import_stamps_the_session_title() {
        let root = tempfile::tempdir().unwrap();
        // A pi `session_info` line is where the title lives; import must carry it onto the session
        // through the message stream, not drop it.
        let body = [
            serde_json::json!({"type": "session", "version": 3, "id": "s2"}).to_string(),
            serde_json::json!({"type": "session_info", "id": "s2-info", "name": "Fix the parser"})
                .to_string(),
            serde_json::json!({"type": "message", "id": "s2-m0",
                "message": {"role": "user", "content": "hi"}})
            .to_string(),
        ]
        .join("\n")
            + "\n";
        std::fs::write(root.path().join("1700000000_s2.jsonl"), body).unwrap();

        let sink =
            Arc::new(Sink::new(mem_store().await, AiSessionDatabase::in_memory().await.unwrap()));
        let _: Vec<_> = SessionImporter::new(sink.clone(), pool())
            .harness(HarnessKind::Pi, pi_sessions(root.path()))
            .collect()
            .await;

        let handle = HarnessSession {
            harness: HarnessKind::Pi,
            session: NativeSessionId::from("s2".to_owned()),
        };
        let session = sink.sidecar.get_session(&handle).await.unwrap().unwrap();
        assert_eq!(session.title.as_deref(), Some("Fix the parser"));
    }

    #[cfg(unix)]
    #[rstest]
    #[tokio::test]
    async fn import_reports_unreadable_directories_as_scan_failures() {
        use std::os::unix::fs::PermissionsExt;

        let root = tempfile::tempdir().unwrap();
        let blocked = root.path().join("blocked");
        std::fs::create_dir(&blocked).unwrap();
        std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o000)).unwrap();
        // Root ignores the mode, so the failure path is unreachable there; skip rather than assert.
        if std::fs::read_dir(&blocked).is_ok() {
            std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o755)).unwrap();
            return;
        }

        let sink =
            Arc::new(Sink::new(mem_store().await, AiSessionDatabase::in_memory().await.unwrap()));
        let events: Vec<_> = SessionImporter::new(sink, pool())
            .harness(HarnessKind::Pi, pi_sessions(root.path()))
            .collect()
            .await;
        // Restore before asserting so the tempdir can be cleaned up regardless of the outcome.
        std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o755)).unwrap();

        let scan_failures: u64 = events
            .iter()
            .map(|event| match event {
                ImportProgress::ScanFailed { failed } => *failed,
                ImportProgress::Session { .. } | ImportProgress::Finished { .. } => 0,
            })
            .sum();
        assert!(scan_failures >= 1, "an unreadable directory must surface as a scan failure");
    }

    #[rstest]
    #[tokio::test]
    async fn nop_facade_import_emits_a_single_empty_summary() {
        let cap = AiHarnessSessionCapture::nop().await;
        let events: Vec<_> = cap.import(None).collect().await;
        assert!(matches!(events.as_slice(), [ImportProgress::Finished {
            sessions: 0,
            imported: 0,
            skipped: 0,
            failed: 0
        }]));
    }
}
