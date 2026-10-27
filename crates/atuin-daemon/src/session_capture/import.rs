use std::sync::Arc;

use atuin_client::ai_session::{Appended, HarnessKind, NativeSessionId};
use atuin_common::harnesstools::AnyHarness;
use atuin_common::harnesstools::session::any::{AnySession, AnySessions};
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

/// How many rows an import stores at a time, a few sessions' worth: each batch is one transaction
/// in either store, which is most of what storing a row costs. Larger ones import a little faster
/// but hold the record store's write lock longer, which shell history waits on.
const BATCH: usize = 1024;

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
            let mut reads = existing
                .map(|item| {
                    let keeps_patches = sink.keeps_patches();
                    async move {
                        // A scan failure (unreadable directory/entry) is not a session; surface it
                        // so the summary reflects a partial backfill instead of a silent one.
                        let session = item.ok()?;
                        // Decoding and enriching a session's lines is CPU work: on a blocking
                        // thread, as the runtime's is the daemon's every other request's. (Not the
                        // pool's: reading the session takes a worker of it.)
                        let read = tokio::task::spawn_blocking(move || {
                            futures::executor::block_on(read(kind, keeps_patches, session))
                        });
                        match read.await {
                            Ok(read) => Some(read),
                            Err(err) => std::panic::resume_unwind(err.into_panic()),
                        }
                    }
                })
                .buffer_unordered(concurrency);

            // Sessions' rows are stored a batch at a time, a session's progress once its are.
            let mut batch = Vec::new();
            let mut sessions = Vec::new();
            loop {
                match reads.next().await {
                    Some(None) => {
                        yield ImportProgress::ScanFailed { failed: 1 };
                        continue;
                    }
                    Some(Some(read)) => {
                        sessions.push((read.session, read.rows.len(), read.failed));
                        batch.extend(read.rows);
                        if batch.len() < BATCH {
                            continue;
                        }
                    }
                    None if sessions.is_empty() => break,
                    None => {}
                }

                // A batch at a time however long a session is, each row's fate `None` if its
                // batch failed.
                let mut appended = Vec::with_capacity(batch.len());
                let mut rows = std::mem::take(&mut batch).into_iter().peekable();
                while rows.peek().is_some() {
                    let part: Vec<_> = rows.by_ref().take(BATCH).collect();
                    let len = part.len();
                    match sink.append_all(part).await {
                        Ok(part) => appended.extend(part.into_iter().map(Some)),
                        Err(_) => appended.extend(std::iter::repeat_n(None, len)),
                    }
                }
                let mut at = 0;
                for (session, count, failed) in std::mem::take(&mut sessions) {
                    let rows = &appended[at..at + count];
                    let new = rows.iter().filter(|a| **a == Some(Appended::New)).count() as u64;
                    let lost = rows.iter().filter(|a| a.is_none()).count() as u64;
                    let (imported, skipped, failed) = (new, count as u64 - new - lost, failed + lost);
                    at += count;
                    yield ImportProgress::Session {
                        harness: kind,
                        session,
                        imported,
                        skipped,
                        failed,
                    };
                }
            }
        }
    }
}

/// Read `session` as live capture would: its rows, and how many of its lines failed to read.
async fn read(kind: HarnessKind, keeps_patches: bool, session: AnySession) -> Read {
    // One enricher per session: it carries that session's bookkeeping (title, timestamps,
    // parent, synthetic ids) across its lines, exactly as live capture does, so a backfilled row
    // matches the captured one.
    let mut enricher = MessageEnricher::new(kind).keeping_patches(keeps_patches);
    let sid = session.id();
    let mut rows = Vec::new();
    let mut failed = 0u64;
    let mut messages = session.read();
    while let Some(message) = messages.next().await {
        match message {
            Ok(message) => rows.extend(enricher.capture(&sid, &message)),
            Err(_) => failed += 1,
        }
    }
    rows.extend(enricher.finish(&sid));
    // A bookkeeping line worth no row (matches live capture) is not counted: it is neither a new
    // record nor a dedupe skip.
    Read {
        session: enricher.handle(&sid).session,
        rows,
        failed,
    }
}

/// One session as an import reads it: its rows, and how many of its lines failed to read.
struct Read {
    session: NativeSessionId,
    rows: Vec<atuin_client::ai_session::Message>,
    failed: u64,
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering::SeqCst;

    use atuin_client::ai_session::{
        AiSessionDatabase, AiSessionStore, HarnessSession, NativeSessionId,
    };
    use atuin_client::record::sqlite_store::SqliteStore;
    use atuin_common::encryption::paseto_v4::Key;
    use atuin_common::harnesstools::pi::session::PiSessions;
    use atuin_domain::record::HostId;
    use futures::StreamExt;
    use futures::future::BoxFuture;
    use rstest::rstest;

    use super::*;
    use crate::session_capture::{AiHarnessSessionCapture, hooks};

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

    /// Sessions stored a batch at a time are each credited with their own rows: one part stored
    /// before (an import cut short) counts what was as skipped.
    #[rstest]
    #[tokio::test]
    async fn import_counts_each_session_in_a_batch_as_its_own() {
        let root = tempfile::tempdir().unwrap();
        write_pi_session(root.path(), "s1", &["user:hi"]);
        let sink =
            Arc::new(Sink::new(mem_store().await, AiSessionDatabase::in_memory().await.unwrap()));
        let import = async || -> Vec<_> {
            SessionImporter::new(sink.clone(), pool())
                .harness(HarnessKind::Pi, pi_sessions(root.path()))
                .collect()
                .await
        };
        import().await;

        write_pi_session(root.path(), "s1", &["user:hi", "assistant:yo"]);
        write_pi_session(root.path(), "s2", &["user:hey", "assistant:hello"]);
        let mut counts: Vec<_> = import()
            .await
            .into_iter()
            .filter_map(|event| match event {
                ImportProgress::Session {
                    session,
                    imported,
                    skipped,
                    ..
                } => Some((session.to_string(), imported, skipped)),
                _ => None,
            })
            .collect();
        counts.sort();
        // The header and the turns.
        assert_eq!(counts, [("s1".to_owned(), 1, 2), ("s2".to_owned(), 3, 0)]);
    }

    /// Counts the record pushes capture makes.
    #[derive(Debug, Default)]
    struct CountPushes(AtomicUsize);

    impl hooks::Hooks for CountPushes {
        fn at(&self, point: hooks::Point) -> BoxFuture<'_, hooks::Fault> {
            if point == hooks::Point::CapturePushing {
                self.0.fetch_add(1, SeqCst);
            }
            Box::pin(async { hooks::Fault::None })
        }
    }

    /// A session longer than a batch is still stored a batch at a time, so no transaction holds
    /// the record store for longer than one batch's takes.
    #[rstest]
    #[tokio::test]
    async fn a_long_session_is_stored_a_batch_at_a_time() {
        let root = tempfile::tempdir().unwrap();
        // With its header, two rows past a batch.
        let turns = vec!["user:hi"; BATCH + 1];
        write_pi_session(root.path(), "s1", &turns);
        let pushes = Arc::new(CountPushes::default());
        let mut sink = Sink::new(mem_store().await, AiSessionDatabase::in_memory().await.unwrap());
        sink.hooks = Some(pushes.clone());

        let counts: Vec<_> = SessionImporter::new(Arc::new(sink), pool())
            .harness(HarnessKind::Pi, pi_sessions(root.path()))
            .filter_map(async |event| match event {
                ImportProgress::Session {
                    imported,
                    skipped,
                    failed,
                    ..
                } => Some((imported, skipped, failed)),
                _ => None,
            })
            .collect()
            .await;

        assert_eq!(counts, [(BATCH as u64 + 2, 0, 0)]);
        assert_eq!(pushes.0.load(SeqCst), 2);
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
