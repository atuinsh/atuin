use std::sync::Arc;

use atuin_client::ai_session::{Appended, HarnessKind, NativeSessionId};
use atuin_common::harnesstools::AnyHarness;
use atuin_common::harnesstools::session::any::{
    AnySession, AnySessions, ObservableExt as _, SessionExt as _, SessionsExt as _,
};
use atuin_common::sync::BlockingPool;
use futures::{Stream, StreamExt};
use tokio_util::task::AbortOnDropHandle;

use super::Sink;
use super::engine::{Start, warm_once};
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
            // Dropped when the import ends, whether it finished or was cancelled.
            let _release = ReleaseMemoryOnDrop;
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
            // Each session is read in a task of its own. A read's warm-up holds capture's locks
            // across its queries, and the loop below stops polling the reads while it stores a
            // batch, which takes the same locks. A read polled only by the loop would then never
            // release them, and the import and live capture would both wait forever. Each task is
            // aborted when the import is dropped, as a read polled by the loop would be.
            let mut reads = existing
                .map(|item| {
                    let sink = sink.clone();
                    AbortOnDropHandle::new(tokio::spawn(async move {
                        // A scan failure (unreadable directory/entry) is not a session; surface it
                        // so the summary reflects a partial backfill instead of a silent one.
                        let session = item.ok()?;
                        let Some(enricher) = begun(&sink, kind, &session).await else {
                            let session = NativeSessionId::from(session.id().to_string());
                            return Some(Read { session, rows: Vec::new(), failed: 1 });
                        };
                        // Decoding and enriching a session's lines is CPU work: on a blocking
                        // thread, as the runtime's is the daemon's every other request's. (Not the
                        // pool's: reading the session takes a worker of it.)
                        let read = tokio::task::spawn_blocking(move || {
                            futures::executor::block_on(read(enricher, session))
                        });
                        match read.await {
                            Ok(read) => Some(read),
                            Err(err) => std::panic::resume_unwind(err.into_panic()),
                        }
                    }))
                })
                .buffer_unordered(concurrency)
                .map(|joined| match joined {
                    Ok(read) => read,
                    Err(err) if err.is_panic() => std::panic::resume_unwind(err.into_panic()),
                    // Cancelled: the runtime is shutting down, and the import with it.
                    Err(_) => None,
                });

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

/// An enricher set up to read `session` from its start, as live capture sets one up: with the
/// parent stored for the session, which a continuation's marker its lines copy can't replace (a
/// restored pi fork's first prompt carries one). `None` when the store can't say what that is:
/// rows read without it could relink the session, so none are stored.
async fn begun(sink: &Sink, kind: HarnessKind, session: &AnySession) -> Option<MessageEnricher> {
    let mut enricher = MessageEnricher::new(kind).keeping_patches(sink.keeps_patches());
    let sid = session.id();
    let warmed = match sink.lock_ready().await {
        Ok(locks) => warm_once(sink, &mut enricher, &sid, Start::Beginning, locks)
            .await
            .map_err(|e| e.to_string()),
        Err(e) => Err(e.to_string()),
    };
    match warmed {
        Ok(()) => Some(enricher),
        Err(e) => {
            tracing::warn!(%e, session = %sid, "can't read an ai-session's stored parent; not importing it");
            None
        }
    }
}

/// Read `session` as live capture would, with `enricher` set up for it ([`begun`]): its rows, and
/// how many of its lines failed to read.
async fn read(mut enricher: MessageEnricher, session: AnySession) -> Read {
    // One enricher per session: it carries that session's bookkeeping (title, timestamps,
    // parent, synthetic ids) across its lines, exactly as live capture does, so a backfilled row
    // matches the captured one.
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

/// Hands the memory an import freed back to the operating system when it is dropped.
///
/// An import reads whole sessions into memory, up to [`SessionImporter`]'s concurrency at a time.
/// The allocator keeps that memory once it is freed. Without this, a large import left the daemon
/// at its peak size, close to a gigabyte, until it restarted.
struct ReleaseMemoryOnDrop;

impl Drop for ReleaseMemoryOnDrop {
    fn drop(&mut self) {
        // Releasing walks every arena, which is too slow for the runtime's own threads.
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            drop(runtime.spawn_blocking(atuin_common::os::memory::release_free_memory));
        }
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
    use atuin_common::harnesstools::copilot::session::CopilotSessions;
    use atuin_common::harnesstools::hermes::session::HermesSessions;
    use atuin_common::harnesstools::kiro::session::KiroSessions;
    use atuin_common::harnesstools::pi::session::PiSessions;
    use atuin_common::harnesstools::session::ParentKind;
    use atuin_common::path::PathBufExt as _;
    use atuin_domain::record::HostId;
    use futures::StreamExt;
    use futures::future::BoxFuture;
    use rstest::rstest;
    use tokio::sync::Semaphore;

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

    /// A pi fork restored from sync without its link, and gone on with while capture was off, is
    /// imported as the fork it is: its first prompt opens with the marker of the continuation it
    /// was forked from, and the parent stored for the session stands over it.
    #[rstest]
    #[tokio::test]
    async fn import_keeps_a_restored_forks_stored_parent() {
        const ORIGINAL: &str = "019a0d14-f276-77d3-b955-89d5b0151306";
        let root = tempfile::tempdir().unwrap();
        let codex = AnyHarness::from_name("codex").unwrap();
        let marker = atuin_common::harnesstools::continuation::marker_text(codex, ORIGINAL, None);
        let write = |parent_session: Option<&str>, turns: &[&str]| {
            let mut header = serde_json::json!({"type": "session", "version": 3, "id": "fork"});
            if let Some(parent) = parent_session {
                header["parentSession"] =
                    serde_json::json!(format!("/s/1690000000_{parent}.jsonl"));
            }
            let first = serde_json::json!({"type": "message", "id": "m0", "message": {
                "role": "user",
                "content": [{"type": "text", "text": marker}, {"type": "text", "text": "go on"}],
            }});
            let rest = turns.iter().enumerate().map(|(i, text)| {
                serde_json::json!({"type": "message", "id": format!("m{}", i + 1),
                    "message": {"role": "user", "content": text}})
            });
            let lines: Vec<String> = std::iter::once(header)
                .chain(std::iter::once(first))
                .chain(rest)
                .map(|line| line.to_string() + "\n")
                .collect();
            std::fs::write(root.path().join("1700000000_fork.jsonl"), lines.concat()).unwrap();
        };
        let sink =
            Arc::new(Sink::new(mem_store().await, AiSessionDatabase::in_memory().await.unwrap()));
        let handle = HarnessSession {
            harness: HarnessKind::Pi,
            session: NativeSessionId::from("fork".to_owned()),
        };
        let import = async || -> Vec<ImportProgress> {
            SessionImporter::new(sink.clone(), pool())
                .harness(HarnessKind::Pi, pi_sessions(root.path()))
                .collect()
                .await
        };
        let continuation = HarnessSession {
            harness: HarnessKind::Pi,
            session: NativeSessionId::from("cont".to_owned()),
        };

        // The fork as pi's `/fork` wrote it, naming the continuation's file.
        write(Some("cont"), &[]);
        import().await;
        // Restored without its link, then gone on with.
        write(None, &["and then?"]);
        let progress = import().await;

        assert_eq!(sum_new(&progress), 1, "only the new turn");
        let session = sink.sidecar.get_session(&handle).await.unwrap().unwrap();
        assert_eq!(session.parent, Some(continuation));
        assert_eq!(session.parent_kind, Some(ParentKind::Fork));
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

    /// Holds the second warm-up inside capture's locks until the first append is about to wait
    /// for them.
    #[derive(Debug)]
    struct HeldWarmUp {
        warm_ups: AtomicUsize,
        appends: AtomicUsize,
        /// A permit once the second warm-up holds capture's locks.
        holding: Semaphore,
        /// A permit once the first append is about to wait for capture's locks.
        appending: Semaphore,
    }

    impl HeldWarmUp {
        fn new() -> Self {
            Self {
                warm_ups: AtomicUsize::new(0),
                appends: AtomicUsize::new(0),
                holding: Semaphore::new(0),
                appending: Semaphore::new(0),
            }
        }
    }

    impl hooks::Hooks for HeldWarmUp {
        fn at(&self, point: hooks::Point) -> BoxFuture<'_, hooks::Fault> {
            Box::pin(async move {
                match point {
                    hooks::Point::WarmRead(hooks::WarmStep::Session)
                        if self.warm_ups.fetch_add(1, SeqCst) == 1 =>
                    {
                        self.holding.add_permits(1);
                        self.appending.acquire().await.unwrap().forget();
                    }
                    hooks::Point::Appending if self.appends.fetch_add(1, SeqCst) == 0 => {
                        self.holding.acquire().await.unwrap().forget();
                        self.appending.add_permits(1);
                    }
                    _ => {}
                }
                hooks::Fault::None
            })
        }
    }

    /// A batch is stored while another session's warm-up holds capture's locks. The import waits
    /// for that warm-up to finish instead of waiting forever for the locks it holds.
    #[rstest]
    #[tokio::test]
    async fn a_batch_stored_during_another_sessions_warm_up_does_not_deadlock() {
        let root = tempfile::tempdir().unwrap();
        // Each session fills a batch on its own. The first one read is stored while the second
        // one's warm-up holds the locks, and that warm-up only goes on once the store is waiting.
        let turns = vec!["user:hi"; BATCH];
        write_pi_session(root.path(), "s1", &turns);
        write_pi_session(root.path(), "s2", &turns);
        let mut sink = Sink::new(mem_store().await, AiSessionDatabase::in_memory().await.unwrap());
        sink.hooks = Some(Arc::new(HeldWarmUp::new()));

        let import = SessionImporter::new(Arc::new(sink), pool())
            .harness(HarnessKind::Pi, pi_sessions(root.path()))
            .collect::<Vec<_>>();
        let progress = tokio::time::timeout(std::time::Duration::from_secs(10), import)
            .await
            .expect("the import deadlocked");

        // Each session's header and turns.
        assert_eq!(sum_new(&progress), 2 * (BATCH as u64 + 1));
    }

    /// Holds every warm-up inside capture's locks until it is dropped.
    #[derive(Debug)]
    struct StuckWarmUps {
        /// A permit for each warm-up that holds capture's locks.
        holding: Semaphore,
    }

    impl hooks::Hooks for StuckWarmUps {
        fn at(&self, point: hooks::Point) -> BoxFuture<'_, hooks::Fault> {
            Box::pin(async move {
                if point == hooks::Point::WarmRead(hooks::WarmStep::Session) {
                    self.holding.add_permits(1);
                    futures::future::pending::<()>().await;
                }
                hooks::Fault::None
            })
        }
    }

    /// An import dropped while a session's warm-up holds capture's locks releases them, so
    /// capture goes on once an import is cancelled.
    #[rstest]
    #[tokio::test]
    async fn a_dropped_import_releases_capture_locks() {
        let root = tempfile::tempdir().unwrap();
        write_pi_session(root.path(), "s1", &["user:hi"]);
        let hooks = Arc::new(StuckWarmUps {
            holding: Semaphore::new(0),
        });
        let mut sink = Sink::new(mem_store().await, AiSessionDatabase::in_memory().await.unwrap());
        sink.hooks = Some(hooks.clone());
        let sink = Arc::new(sink);

        let mut import = Box::pin(
            SessionImporter::new(sink.clone(), pool())
                .harness(HarnessKind::Pi, pi_sessions(root.path())),
        );
        let held = async {
            tokio::select! {
                event = import.next() => panic!("the import went on past a held warm-up: {event:?}"),
                permit = hooks.holding.acquire() => permit.unwrap().forget(),
            }
        };
        tokio::time::timeout(std::time::Duration::from_secs(10), held)
            .await
            .expect("the warm-up never took capture's locks");
        drop(import);

        let locks = tokio::time::timeout(std::time::Duration::from_secs(5), sink.lock_ready())
            .await
            .expect("the dropped import kept capture's locks");
        assert!(locks.is_ok());
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

    /// A Copilot session's name lives in the sidecar beside its log, and its directory and
    /// branch in the log's first event: import carries all three onto the session.
    #[rstest]
    #[tokio::test]
    async fn import_reads_a_copilot_sessions_sidecar_and_context() {
        let root = tempfile::tempdir().unwrap();
        let id = "8623b9dc-b2aa-40dc-a8ce-1f3738d4d4db";
        let dir = root.path().join(id);
        std::fs::create_dir(&dir).unwrap();
        std::fs::write(
            dir.join("events.jsonl"),
            include_str!("../../../atuin-common/tests/fixtures/copilot/task-and-edit.jsonl"),
        )
        .unwrap();
        std::fs::write(
            dir.join("workspace.yaml"),
            include_str!(
                "../../../atuin-common/tests/fixtures/copilot/task-and-edit.workspace.yaml"
            ),
        )
        .unwrap();

        let sink =
            Arc::new(Sink::new(mem_store().await, AiSessionDatabase::in_memory().await.unwrap()));
        let sessions = AnySessions::from(
            CopilotSessions::builder().root(root.path().to_path_buf()).pool(pool()).build(),
        );
        let progress: Vec<_> = SessionImporter::new(sink.clone(), pool())
            .harness(HarnessKind::Copilot, sessions)
            .collect()
            .await;
        assert_eq!(
            sum_new(&progress),
            13 + 1 + 4,
            "the parent's rows and its title, and its subagent's rows"
        );

        let handle = HarnessSession {
            harness: HarnessKind::Copilot,
            session: NativeSessionId::from(id.to_owned()),
        };
        let session = sink.sidecar.get_session(&handle).await.unwrap().unwrap();
        assert_eq!(session.title.as_deref(), Some("atuin-sample-edit"));
        assert_eq!(session.cwd.as_deref(), Some(Path::new("/tmp/copilot-play")));
        assert_eq!(session.git_branch.as_deref(), Some("main"));

        // The subagent is a session of its own, a child of the parent named for its agent.
        let child = HarnessSession {
            harness: HarnessKind::Copilot,
            session: NativeSessionId::from("agent-toolu_01Cz14Gbx74QYDbB2WUL3D5T".to_owned()),
        };
        let subagent = sink.sidecar.get_session(&child).await.unwrap().unwrap();
        assert_eq!(subagent.title.as_deref(), Some("count-lines"));
        assert_eq!(subagent.parent.as_ref(), Some(&handle));
        assert_eq!(subagent.parent_kind, Some(ParentKind::Subagent));
    }

    /// A Kiro session's title, directory and model live in the header beside its log: import
    /// carries them onto the session, and reads the rows the log holds.
    #[rstest]
    #[tokio::test]
    async fn import_reads_a_kiro_sessions_header_and_rows() {
        let root = tempfile::tempdir().unwrap();
        let id = "sess_7c362b91-6e72-4c10-be1f-76a31a8d5f48";
        let dir = root.path().join("c15e787aea4828dd").joined(id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("messages.jsonl"),
            include_str!("../../../atuin-common/tests/fixtures/kiro/resumed.jsonl"),
        )
        .unwrap();
        std::fs::write(
            dir.join("session.json"),
            include_str!("../../../atuin-common/tests/fixtures/kiro/resumed.session.json"),
        )
        .unwrap();

        let sink =
            Arc::new(Sink::new(mem_store().await, AiSessionDatabase::in_memory().await.unwrap()));
        let sessions = AnySessions::from(
            KiroSessions::builder().root(root.path().to_path_buf()).pool(pool()).build(),
        );
        let progress: Vec<_> = SessionImporter::new(sink.clone(), pool())
            .harness(HarnessKind::Kiro, sessions)
            .collect()
            .await;
        assert_eq!(sum_new(&progress), 19 + 1, "the conversation rows and the title");

        let handle = HarnessSession {
            harness: HarnessKind::Kiro,
            session: NativeSessionId::from(id.to_owned()),
        };
        let session = sink.sidecar.get_session(&handle).await.unwrap().unwrap();
        assert_eq!(session.title.as_deref(), Some("Read README and Run Probe Command"));
        assert_eq!(session.cwd.as_deref(), Some(Path::new("/tmp/kiro-play")));
        assert_eq!(session.git_branch, None);
    }

    /// Hermes keeps every session in one `state.db`: import reads its coding sessions, with each
    /// one's directory and the rows the store holds, and the subagent the probe delegated to, a
    /// child of the session that delegated.
    #[rstest]
    #[tokio::test]
    async fn import_reads_hermes_sessions_from_its_store() {
        let home = tempfile::tempdir().unwrap();
        let db = home.path().join("state.db");
        let mut conn = <sqlx::SqliteConnection as sqlx::Connection>::connect_with(
            &sqlx::sqlite::SqliteConnectOptions::new()
                .filename(&db)
                .create_if_missing(true)
                .foreign_keys(false),
        )
        .await
        .unwrap();
        for sql in [
            include_str!("../../../atuin-common/tests/fixtures/hermes/schema.sql"),
            include_str!("../../../atuin-common/tests/fixtures/hermes/probe.sql"),
        ] {
            // Not through `atuin_common::db::query`, which takes only static SQL: this stands for
            // Hermes's own writes.
            #[allow(clippy::disallowed_methods)]
            sqlx::raw_sql(sqlx::AssertSqlSafe(sql)).execute(&mut conn).await.unwrap();
        }
        sqlx::Connection::close(conn).await.unwrap();

        let sink =
            Arc::new(Sink::new(mem_store().await, AiSessionDatabase::in_memory().await.unwrap()));
        let sessions = AnySessions::from(
            HermesSessions::builder().home(home.path().to_path_buf()).pool(pool()).build(),
        );
        let progress: Vec<_> = SessionImporter::new(sink.clone(), pool())
            .harness(HarnessKind::Hermes, sessions)
            .collect()
            .await;
        // Rows 1-28: the subagent's are 23-26.
        assert_eq!(sum_new(&progress), 28);

        let handle = HarnessSession {
            harness: HarnessKind::Hermes,
            session: NativeSessionId::from(
                "local-524543fc0c9eeda01f40f2cd88fefc201a75a0fd59817a174c4f18e0ebb80136".to_owned(),
            ),
        };
        let session = sink.sidecar.get_session(&handle).await.unwrap().unwrap();
        assert_eq!(session.cwd.as_deref(), Some(Path::new("/tmp/hermes-play/work")));
        let delegator = HarnessSession {
            harness: HarnessKind::Hermes,
            session: NativeSessionId::from(
                "local-56e64527431b680fad9f17881150e9234ddbca30777b3d57949bd767d30315e1".to_owned(),
            ),
        };
        let subagent = HarnessSession {
            harness: HarnessKind::Hermes,
            session: NativeSessionId::from("20261109_094652_9ca0a5".to_owned()),
        };
        let subagent = sink.sidecar.get_session(&subagent).await.unwrap().unwrap();
        assert_eq!(subagent.parent.as_ref(), Some(&delegator));
        assert_eq!(subagent.parent_kind, Some(ParentKind::Subagent));
        assert_eq!(subagent.cwd.as_deref(), Some(Path::new("/tmp/hermes-play/work")));
    }

    /// A Kiro subagent's log, under its parent's directory, is a session of its own: import
    /// reads it beside the parent, a child named for its agent, working where the parent does.
    #[rstest]
    #[tokio::test]
    async fn import_reads_a_kiro_sessions_subagent() {
        let root = tempfile::tempdir().unwrap();
        let id = "sess_d3a5510d-40b8-4a77-be1c-bf31a17f584d";
        let subagent = "5d13f3d7-73ae-431d-847f-04729b40503e";
        let dir = root.path().join("01fceef077f333f9").joined(id);
        std::fs::create_dir_all(dir.join("sub-executions")).unwrap();
        std::fs::write(
            dir.join("messages.jsonl"),
            include_str!("../../../atuin-common/tests/fixtures/kiro/compacted.jsonl"),
        )
        .unwrap();
        std::fs::write(
            dir.join("session.json"),
            include_str!("../../../atuin-common/tests/fixtures/kiro/compacted.session.json"),
        )
        .unwrap();
        std::fs::write(
            dir.join("sub-executions").joined(format!("{subagent}.jsonl")),
            include_str!("../../../atuin-common/tests/fixtures/kiro/compacted.subagent.jsonl"),
        )
        .unwrap();

        let sink =
            Arc::new(Sink::new(mem_store().await, AiSessionDatabase::in_memory().await.unwrap()));
        let sessions = AnySessions::from(
            KiroSessions::builder().root(root.path().to_path_buf()).pool(pool()).build(),
        );
        let progress: Vec<_> = SessionImporter::new(sink.clone(), pool())
            .harness(HarnessKind::Kiro, sessions)
            .collect()
            .await;
        assert_eq!(
            sum_new(&progress),
            25 + 1 + 7 + 1,
            "the parent's rows and its title, and the subagent's rows and its name"
        );

        let handle = HarnessSession {
            harness: HarnessKind::Kiro,
            session: NativeSessionId::from(id.to_owned()),
        };
        let session = sink.sidecar.get_session(&handle).await.unwrap().unwrap();
        assert_eq!(session.title.as_deref(), Some("Read README and Run Unit Tests"));
        assert_eq!(session.parent, None);

        let child = HarnessSession {
            harness: HarnessKind::Kiro,
            session: NativeSessionId::from(subagent.to_owned()),
        };
        let subagent = sink.sidecar.get_session(&child).await.unwrap().unwrap();
        assert_eq!(subagent.title.as_deref(), Some("kit-reviewer"));
        assert_eq!(subagent.cwd.as_deref(), Some(Path::new("/work/20261008T150143Z-K18-v3")));
        assert_eq!(subagent.parent.as_ref(), Some(&handle));
        assert_eq!(subagent.parent_kind, Some(ParentKind::Subagent));
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
