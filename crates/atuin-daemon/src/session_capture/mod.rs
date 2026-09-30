#[cfg(test)]
mod chaos_tests;
mod engine;
#[cfg(test)]
pub(crate) mod hooks;
mod import;
mod message_enricher;
mod recovery;

use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::Duration;

use atuin_client::ai_session::{
    AiSessionDatabase, AiSessionStore, Appended, DbError, HarnessKind, HarnessSession, Message,
    PushError, ReprojectProgress, SearchTerms, Session, SessionFilter, SessionMatch,
};
use atuin_client::record::sqlite_store::SqliteStore;
use atuin_common::encryption::paseto_v4::Key;
use atuin_common::harnesstools::session::{Content, Role};
use atuin_common::sync::BlockingPool;
use atuin_domain::record::HostId;
use engine::SessionCaptureEngine;
use futures::{FutureExt, Stream, StreamExt};
pub use import::ImportProgress;
use import::SessionImporter;
use recovery::{Coordinator, Msg};
use tokio::sync::{Mutex, broadcast, mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tokio_stream::wrappers::BroadcastStream;

const NOP_STORE_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone)]
pub enum SessionTailEvent {
    SessionStarted(Session),
    SessionUpdated(Session),
    Message(Message),
}

#[derive(Debug, thiserror::Error)]
pub enum AppendError {
    #[error(transparent)]
    Sidecar(#[from] DbError),
    #[error(transparent)]
    Push(#[from] PushError),
    /// The store is unavailable (its recovery or a rebuild failed): its sidecar may be missing
    /// persisted messages, so the dedup gate cannot be trusted and nothing is written.
    #[error("the AI session store is unavailable")]
    Unavailable,
}

#[derive(Debug, thiserror::Error)]
pub enum RebuildError {
    /// Opening or recovering the store failed at startup: there is nothing to rebuild with until
    /// the daemon restarts, which rebuilds anyway. (A store that recovered but whose rebuild then
    /// failed can be rebuilt again.)
    #[error("the AI session store is unavailable: it failed to open or recover at startup")]
    Unavailable,
    #[error(transparent)]
    Sidecar(#[from] DbError),
    /// The rebuild panicked, or the runtime is shutting down.
    #[error("the AI session rebuild stopped unexpectedly")]
    Aborted,
}

pub(crate) struct Sink {
    records: AiSessionStore,
    sidecar: AiSessionDatabase,
    tail: broadcast::Sender<SessionTailEvent>,
    // Serialize the dedup gate + persistence across live capture and import. At most one record
    // can be waiting for projection; repair it before admitting another capture.
    pending_projection: Mutex<Option<Message>>,
    /// The store's state: capture and import wait out a rebuild, and are refused while the store
    /// is unavailable (see `append`).
    state: watch::Receiver<StoreState>,
    #[cfg(test)]
    hooks: Option<Arc<dyn hooks::Hooks>>,
}

impl Sink {
    #[cfg(test)]
    pub(crate) fn new(records: AiSessionStore, sidecar: AiSessionDatabase) -> Self {
        Self::with_state(records, sidecar, watch::channel(StoreState::Ready).1)
    }

    fn with_state(
        records: AiSessionStore,
        sidecar: AiSessionDatabase,
        state: watch::Receiver<StoreState>,
    ) -> Self {
        let (tail, _) = broadcast::channel(128);
        Self {
            records,
            sidecar,
            tail,
            pending_projection: Mutex::new(None),
            state,
            #[cfg(test)]
            hooks: None,
        }
    }

    #[cfg(test)]
    async fn hook(&self, point: hooks::Point) -> hooks::Fault {
        match &self.hooks {
            Some(hooks) => hooks.at(point).await,
            None => hooks::Fault::None,
        }
    }

    #[cfg(test)]
    fn enter(&self, section: hooks::Section) -> Option<Box<dyn Send>> {
        self.hooks.as_ref().map(|hooks| hooks.enter(section))
    }

    pub(crate) fn subscribe(&self) -> BroadcastStream<SessionTailEvent> {
        BroadcastStream::new(self.tail.subscribe())
    }

    /// Persist and project `msg`, unless its logical message is already projected.
    ///
    /// Only while the store is [`StoreState::Ready`]: this waits out a rebuild, and refuses with
    /// [`AppendError::Unavailable`] (writing nothing) while the store is unavailable, as the
    /// sidecar the dedup gate trusts may then be missing persisted messages.
    pub(crate) async fn append(&self, mut msg: Message) -> Result<Appended, AppendError> {
        // Apply capture policy before either persistence path or the live tail. Keep structural
        // rows (even with no content) so parent links and usage accounting remain intact.
        sanitize_message(&mut msg);
        // Captured here, so on this host; a reproject reads the same from the record envelope.
        msg.host = Some(self.records.host_id());
        let mut state = self.state.clone();
        let (mut pending, local) = loop {
            // The dedup gate below trusts the sidecar, which a rebuild empties and refills:
            // checked meanwhile, a message already in the record store would be pushed again. So
            // wait out a rebuild (capture and import only start once startup recovery is over),
            // and refuse while the store is unavailable: a failed replay leaves a sidecar that
            // may be missing persisted messages.
            if Self::settled(&mut state).await != StoreState::Ready {
                return Err(AppendError::Unavailable);
            }
            #[cfg(test)]
            self.hook(hooks::Point::CaptureWaited).await;
            let pending = self.pending_projection.lock().await;
            // Keeps a reprojection of this host's records (after a sync) from projecting the
            // record pushed below before this does, and a rebuild from emptying the sidecar
            // between the check below and the push. Taken after `pending`, never the other way
            // round.
            let local = self.sidecar.lock_local_projection().await;
            // Checked again under the lock: a rebuild says it is recovering before it takes the
            // lock to empty the sidecar, so one that began while this waited for the locks may
            // have emptied it already. Ready here, no rebuild can empty it until this is done.
            let now = *state.borrow();
            match now {
                StoreState::Ready => break (pending, local),
                StoreState::Recovering => {
                    #[cfg(test)]
                    self.hook(hooks::Point::CaptureOvertaken).await;
                }
                StoreState::Unavailable => return Err(AppendError::Unavailable),
            }
        };
        #[cfg(test)]
        let section = self.enter(hooks::Section::Capture);
        #[cfg(test)]
        self.hook(hooks::Point::CaptureChecked).await;
        if let Some(previous) = pending.as_ref() {
            self.project_and_broadcast(previous).await?;
            *pending = None;
        }
        // Dedup gate: if this logical message is already projected it is already in the record
        // store too, so there is nothing to do. Stable source ids (see MessageEnricher::source_id)
        // make this reliable across re-captures and keep the record store free of duplicates.
        if self.sidecar.contains_message(&msg.session, &msg.source_id).await? {
            return Ok(Appended::Duplicate);
        }

        // Write the record store first: it is the synced source of truth and the sidecar is a
        // pure projection of it. If the sidecar write fails afterwards a later rebuild repairs it;
        // the reverse ordering could strand a message in the sidecar only -- lost on rebuild and
        // never synced.
        self.records.push(&msg).await?;

        *pending = Some(msg.clone());
        let appended = self.project_and_broadcast(&msg).await?;
        *pending = None;
        #[cfg(test)]
        drop(section);
        drop(local);
        drop(pending);
        Ok(appended)
    }

    /// Wait out a rebuild or recovery, and say how it ended. A closed channel (the coordinator
    /// is gone, so the state can no longer change) reads as the state it was left in, with a
    /// recovery that never ended as unavailable.
    async fn settled(state: &mut watch::Receiver<StoreState>) -> StoreState {
        let waited = state.wait_for(|state| *state != StoreState::Recovering).await.map(|s| *s);
        let settled = waited.unwrap_or_else(|_| *state.borrow());
        if settled == StoreState::Recovering {
            StoreState::Unavailable
        } else {
            settled
        }
    }

    async fn project_and_broadcast(&self, msg: &Message) -> Result<Appended, AppendError> {
        let started = self.sidecar.get_session(&msg.session).await?.is_none();
        let appended = self.sidecar.append(msg).await?;
        if self.tail.receiver_count() > 0 {
            if let Some(session) = self.sidecar.get_session(&msg.session).await? {
                let event = if started {
                    SessionTailEvent::SessionStarted(session)
                } else {
                    SessionTailEvent::SessionUpdated(session)
                };
                let _ = self.tail.send(event);
            }
            let _ = self.tail.send(SessionTailEvent::Message(msg.clone()));
        }

        Ok(appended)
    }
}

/// Retain conversation text and payload-free tool breadcrumbs, never execution payloads.
/// Null payloads preserve the existing wire format without storing arguments or results.
/// This only affects new captures; existing synced records are not rewritten.
fn sanitize_message(msg: &mut Message) {
    let conversation = matches!(msg.role, Role::User | Role::Assistant);
    msg.content.retain_mut(|block| match block {
        Content::Text(text) if conversation => {
            *text = atuin_common::secrets::redact(text).into_owned();
            true
        }
        Content::ToolUse(tool) => {
            tool.input = serde_json::Value::Null;
            true
        }
        Content::ToolResult(result) => {
            result.output = serde_json::Value::Null;
            true
        }
        Content::Reasoning(_) => {
            *block = Content::ReasoningSummary { tokens: None };
            true
        }
        // Model-written summaries and failure reasons are conversation, whatever the role.
        Content::Summary(text) | Content::Error(text) => {
            *text = atuin_common::secrets::redact(text).into_owned();
            true
        }
        Content::ReasoningSummary { .. } => true,
        Content::Text(_) | Content::Other(_) => false,
    });
    // Both titles a row carries: the session's, and the one its own line set.
    let change = msg.title_change.as_mut().and_then(|change| change.text.as_mut());
    for title in msg.session_title.iter_mut().chain(change) {
        *title = atuin_common::secrets::redact(title).into_owned();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StoreState {
    /// Startup recovery or a rebuild is reprojecting the sidecar; reads see what is projected so
    /// far.
    Recovering,
    /// The sidecar holds every record: capture (when enabled) and import are running.
    Ready,
    /// Opening or reprojecting the store failed; capture and import are refused (see
    /// [`Sink::append`]) until restart, or, once startup recovery had succeeded, until a rebuild
    /// succeeds.
    Unavailable,
}

/// Resolves once startup recovery is over, whether it succeeded or not.
#[derive(Debug, Clone)]
pub struct Recovery {
    state: watch::Receiver<StoreState>,
    /// The facade's test hooks, for the sync worker's (see its `SyncReproject` point).
    #[cfg(test)]
    pub(crate) hooks: Option<Arc<dyn hooks::Hooks>>,
}

impl Recovery {
    pub async fn finished(&self) {
        // A closed channel means the coordinator is gone: recovery is over either way.
        let _ = self.state.clone().wait_for(|state| *state != StoreState::Recovering).await;
    }
}

pub struct AiHarnessSessionCapture {
    sink: Arc<Sink>,
    /// Runs the harness session file reads of capture and import.
    pool: BlockingPool,
    /// Written by the coordinator only (see [`recovery`]).
    state: watch::Receiver<StoreState>,
    /// The coordinator's mailbox, for rebuilds. None without a persistent store.
    coordinator: Option<mpsc::UnboundedSender<Msg>>,
    /// How far the replay running has got.
    progress: ReprojectProgress,
    /// The coordinator, and the capture engine once startup recovery succeeds; aborted on drop.
    background: Vec<JoinHandle<()>>,
}

impl Drop for AiHarnessSessionCapture {
    fn drop(&mut self) {
        for task in &self.background {
            task.abort();
        }
    }
}

impl AiHarnessSessionCapture {
    /// Serve the sidecar immediately and recover it from the record store in the background.
    ///
    /// Recovery is [`AiSessionStore::reproject`]: only the records past each series' watermark
    /// are replayed, and everything when the sidecar is fresh (a new install, or one another
    /// build's migrations made and [`AiSessionDatabase::open`] set aside) or its watermarks were
    /// cleared (a migration that needs a backfill, a key change).
    ///
    /// Capture and import wait for recovery: the dedup gate trusts the sidecar, so writing before
    /// it holds every persisted message would push duplicate records. So does the sync worker's
    /// projection of downloaded records (see [`Recovery`]), so one reprojection runs at a time.
    /// Failed recovery leaves existing sessions readable but keeps both off until restart.
    #[must_use]
    pub fn open(
        records: AiSessionStore,
        sidecar: AiSessionDatabase,
        capture: bool,
        pool: BlockingPool,
    ) -> Self {
        Self::open_with(
            records,
            sidecar,
            capture,
            pool,
            #[cfg(test)]
            None,
        )
    }

    fn open_with(
        records: AiSessionStore,
        sidecar: AiSessionDatabase,
        capture: bool,
        pool: BlockingPool,
        #[cfg(test)] hooks: Option<Arc<dyn hooks::Hooks>>,
    ) -> Self {
        let (state_tx, state) = watch::channel(StoreState::Recovering);
        #[cfg_attr(not(test), expect(unused_mut))]
        let mut sink = Sink::with_state(records, sidecar, state.clone());
        #[cfg(test)]
        {
            sink.hooks = hooks;
        }
        let sink = Arc::new(sink);
        let progress = ReprojectProgress::default();
        let (coordinator, coordinating) =
            Coordinator::spawn(sink.clone(), state_tx, progress.clone());

        // Capture is opt-in. When disabled we still serve existing sessions, but never spawn the
        // listeners that copy new transcripts into the synced record store. It starts once the
        // store is first ready: never, if startup recovery failed, as rebuilds are refused then.
        // (Not on the first state after recovering: a rebuild could have left the store
        // unavailable by the time this looks.)
        let capturing = tokio::spawn({
            let sink = sink.clone();
            let pool = pool.clone();
            let mut state = state.clone();
            async move {
                let ready = state.wait_for(|state| *state == StoreState::Ready).await.is_ok();
                let _engine = if capture && ready {
                    SessionCaptureEngine::spawn(&sink, &pool)
                } else {
                    SessionCaptureEngine::nop()
                };
                std::future::pending::<()>().await;
            }
        });

        Self {
            sink,
            pool,
            state,
            coordinator: Some(coordinator),
            progress,
            background: vec![coordinating, capturing],
        }
    }

    pub async fn nop() -> Self {
        Self::nop_in(watch::channel(StoreState::Unavailable).1).await
    }

    /// A nop facade whose recovery state the test drives.
    #[cfg(test)]
    pub(crate) async fn with_state(state: StoreState) -> (Self, watch::Sender<StoreState>) {
        let (tx, rx) = watch::channel(state);
        (Self::nop_in(rx).await, tx)
    }

    async fn nop_in(state: watch::Receiver<StoreState>) -> Self {
        let store = SqliteStore::in_memory(NOP_STORE_TIMEOUT)
            .await
            .expect("in-memory sqlite store must open for the nop ai-session capture facade");
        let records = AiSessionStore::builder()
            .store(store)
            .host_id(HostId(atuin_common::utils::uuid_v7()))
            .key(Key::generate())
            .build();
        let sidecar = AiSessionDatabase::in_memory().await.expect(
            "in-memory ai-session database must open for the nop ai-session capture facade",
        );

        Self {
            sink: Arc::new(Sink::with_state(records, sidecar, state.clone())),
            // Never runs anything: without a persistent store there is no capture or import.
            pool: BlockingPool::new(NonZeroUsize::MIN),
            state,
            // No coordinator: nothing to rebuild, and nothing sets the state a test drives but
            // the test.
            coordinator: None,
            progress: ReprojectProgress::default(),
            background: Vec::new(),
        }
    }

    /// Rebuild the sidecar from the record store while serving, for the maintenance commands
    /// that delete records under it (`atuin store purge`, `atuin store pull --force`) or ask for
    /// a rebuild (`atuin store rebuild ai-session`): a reprojection only adds, so what deleted
    /// records projected would otherwise stay.
    ///
    /// Everything projected is deleted ([`AiSessionDatabase::reset`]) before this returns, then
    /// replayed in the background, recovering as at startup meanwhile: reads are refused as
    /// rebuilding with the replay's progress, and capture and import wait (their dedup gate
    /// trusts the sidecar). During startup recovery, or another rebuild, the replay running
    /// replays again after this reset before the store is ready (see [`recovery`]).
    ///
    /// Runs to the end once asked for, even if the caller stops waiting (a client
    /// disconnecting): the coordinator carries it out, not the caller.
    pub async fn rebuild(&self) -> Result<(), RebuildError> {
        let Some(coordinator) = &self.coordinator else {
            return Err(RebuildError::Unavailable);
        };
        let (reply, answer) = oneshot::channel();
        if coordinator.send(Msg::Rebuild(reply)).is_err() {
            return Err(RebuildError::Aborted);
        }
        answer.await.unwrap_or(Err(RebuildError::Aborted))
    }

    /// A handle for waiting out startup recovery.
    #[must_use]
    pub fn recovery(&self) -> Recovery {
        Recovery {
            state: self.state.clone(),
            #[cfg(test)]
            hooks: self.sink.hooks.clone(),
        }
    }

    /// Whether startup recovery or a rebuild is still projecting the sidecar, so reads may miss
    /// sessions and messages it has not restored yet. A closed channel means the coordinator is
    /// gone (it panicked, or the facade is going): nothing is in progress.
    #[must_use]
    pub fn is_recovering(&self) -> bool {
        *self.state.borrow() == StoreState::Recovering && self.state.has_changed().is_ok()
    }

    /// While [recovering](Self::is_recovering): the records replayed so far, and roughly how many
    /// there are to replay (see [`ReprojectProgress::get`]).
    #[must_use]
    pub fn recovery_progress(&self) -> (u64, u64) {
        self.progress.get()
    }

    /// Whether recovery succeeded and the persistent session store is ready for capture and
    /// import. `false` while recovering, or after opening or recovering the store failed.
    #[must_use]
    pub fn is_available(&self) -> bool {
        *self.state.borrow() == StoreState::Ready
    }

    /// Wait out startup recovery, then report whether the persistent session store is ready for
    /// capture and import. `false` means opening or recovering the store failed; any projected
    /// sessions remain readable.
    pub async fn ready(&self) -> bool {
        Self::wait_ready(self.state.clone()).await
    }

    async fn wait_ready(mut state: watch::Receiver<StoreState>) -> bool {
        // A closed channel means recovery panicked or was aborted.
        state
            .wait_for(|state| *state != StoreState::Recovering)
            .await
            .is_ok_and(|state| *state == StoreState::Ready)
    }

    pub fn import(
        &self,
        harness: Option<HarnessKind>,
    ) -> impl Stream<Item = ImportProgress> + Send + 'static {
        let state = self.state.clone();
        let sink = self.sink.clone();
        let pool = self.pool.clone();
        async move {
            if Self::wait_ready(state).await {
                SessionImporter::new(sink, pool).run(harness).right_stream()
            } else {
                futures::stream::once(async {
                    ImportProgress::Finished {
                        sessions: 0,
                        imported: 0,
                        skipped: 0,
                        failed: 0,
                    }
                })
                .left_stream()
            }
        }
        .flatten_stream()
    }

    #[must_use]
    pub fn subscribe(&self) -> BroadcastStream<SessionTailEvent> {
        self.sink.subscribe()
    }

    pub async fn list_sessions(&self, filter: &SessionFilter) -> Result<Vec<Session>, DbError> {
        self.sink.sidecar.list_sessions(filter).await
    }

    pub async fn get_session(&self, session: &HarnessSession) -> Result<Option<Session>, DbError> {
        self.sink.sidecar.get_session(session).await
    }

    pub fn messages(
        &self,
        session: &HarnessSession,
    ) -> impl Stream<Item = Result<Message, DbError>> + Send + 'static {
        self.sink.sidecar.messages(session)
    }

    pub fn transcript(
        &self,
        session: &HarnessSession,
    ) -> impl Stream<Item = Result<String, DbError>> + Send + 'static {
        self.sink.sidecar.transcript(session)
    }

    pub fn search(
        &self,
        query: &str,
        terms: SearchTerms,
        filter: &SessionFilter,
        limit: u32,
    ) -> impl Stream<Item = Result<SessionMatch, DbError>> + Send + 'static {
        self.sink.sidecar.search(query, terms, filter, limit)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};

    use atuin_client::ai_session::{NativeSessionId, SourceId};
    use atuin_common::harnesstools::session::{
        TitleChange, TitleSource, ToolCallId, ToolResult, ToolUse, Usage,
    };
    use atuin_domain::record::{RecordId, RecordTag};
    use futures::StreamExt;
    use rstest::rstest;
    use time::OffsetDateTime;

    use super::*;

    async fn mem_store() -> AiSessionStore {
        let store = SqliteStore::in_memory(NOP_STORE_TIMEOUT).await.unwrap();
        AiSessionStore::builder()
            .store(store)
            .host_id(HostId(atuin_common::utils::uuid_v7()))
            .key(Key::generate())
            .build()
    }

    fn sample_handle() -> HarnessSession {
        HarnessSession {
            harness: HarnessKind::ClaudeCode,
            session: NativeSessionId::from("native-session".to_owned()),
        }
    }

    fn sample_message() -> Message {
        Message::builder()
            .id(RecordId(atuin_common::utils::uuid_v7()))
            .session(sample_handle())
            .source_id(SourceId::from("source-id".to_owned()))
            .timestamp(OffsetDateTime::UNIX_EPOCH)
            .role(Role::User)
            .content(vec![Content::Text("hello".to_owned())])
            .build()
    }

    #[rstest]
    #[tokio::test]
    async fn append_emits_started_then_message_to_subscriber() {
        let sink = Sink::new(mem_store().await, AiSessionDatabase::in_memory().await.unwrap());
        let mut sub = sink.subscribe();

        sink.append(sample_message()).await.unwrap();

        assert!(matches!(sub.next().await.unwrap().unwrap(), SessionTailEvent::SessionStarted(_)));
        assert!(matches!(sub.next().await.unwrap().unwrap(), SessionTailEvent::Message(_)));
    }

    #[rstest]
    #[case(Role::User)]
    #[case(Role::Assistant)]
    #[tokio::test]
    async fn capture_sanitizes_records_sidecar_and_tail(#[case] role: Role) {
        let sink = Sink::new(mem_store().await, AiSessionDatabase::in_memory().await.unwrap());
        let mut sub = sink.subscribe();
        let mut msg = sample_message();
        msg.role = role;
        msg.parent_source_id = Some("parent".to_owned().into());
        msg.turn_id = Some("turn".to_owned());
        msg.usage = Some(Usage {
            input: Some(42),
            output: Some(0),
            cache_read: Some(0),
            cache_write: Some(0),
            reasoning: None,
        });
        msg.session_title = Some("AWS_SECRET_ACCESS_KEY=TITLESECRET".to_owned());
        msg.title_change =
            Some(TitleChange::new(TitleSource::Named, "AWS_SECRET_ACCESS_KEY=CHANGESECRET"));
        msg.content = vec![
            Content::Text("AWS_SECRET_ACCESS_KEY=TEXTSECRET".to_owned()),
            Content::ToolUse(ToolUse {
                id: ToolCallId::from("call".to_owned()),
                name: "Bash".to_owned(),
                input: serde_json::json!({"command": "PRIVATE_INPUT"}),
            }),
            Content::ToolResult(ToolResult {
                call: ToolCallId::from("call".to_owned()),
                output: serde_json::json!({"text": "PRIVATE_OUTPUT"}),
                error: true,
            }),
            Content::Reasoning("PRIVATE_REASONING".to_owned()),
            Content::Other(serde_json::json!({"attachment": "PRIVATE_ATTACHMENT"})),
        ];
        sink.append(msg.clone()).await.unwrap();
        sanitize_message(&mut msg);
        // Capture stamps the local host.
        msg.host = Some(sink.records.host_id());
        assert_eq!(msg.content.len(), 4);
        assert_eq!(msg.content[3], Content::ReasoningSummary { tokens: None });
        assert_eq!(msg.content[0], Content::Text("AWS_SECRET_ACCESS_KEY=****".to_owned()));
        assert!(matches!(&msg.content[1], Content::ToolUse(t)
            if t.name == "Bash" && t.id.as_ref() == "call" && t.input.is_null()));
        assert!(matches!(&msg.content[2], Content::ToolResult(t)
            if t.call.as_ref() == "call" && t.error && t.output.is_null()));
        assert_eq!(msg.session_title.as_deref(), Some("AWS_SECRET_ACCESS_KEY=****"));
        assert_eq!(
            msg.title_change.as_ref().and_then(|t| t.text.as_deref()),
            Some("AWS_SECRET_ACCESS_KEY=****"),
        );

        let event = sub.next().await.unwrap().unwrap();
        assert!(matches!(event, SessionTailEvent::SessionStarted(_)));
        let SessionTailEvent::Message(tail) = sub.next().await.unwrap().unwrap() else {
            panic!("expected message");
        };
        assert_eq!(tail, msg);
        assert_eq!(
            sink.sidecar.get_session(&msg.session).await.unwrap().unwrap().title,
            msg.session_title,
        );
        // The projection keeps titles on sessions rather than individual messages.
        let title = msg.session_title.take();
        let mut messages = Box::pin(sink.sidecar.messages(&msg.session));
        assert_eq!(messages.next().await.unwrap().unwrap(), msg);

        // Rebuilding from encrypted records must not restore discarded payloads.
        let rebuilt = AiSessionDatabase::in_memory().await.unwrap();
        sink.records.build(&rebuilt).await.unwrap();
        assert_eq!(rebuilt.get_session(&msg.session).await.unwrap().unwrap().title, title);
        let mut messages = Box::pin(rebuilt.messages(&msg.session));
        assert_eq!(messages.next().await.unwrap().unwrap(), msg);
        for query in [
            "PRIVATE_INPUT",
            "PRIVATE_OUTPUT",
            "PRIVATE_REASONING",
            "PRIVATE_ATTACHMENT",
            "TEXTSECRET",
        ] {
            let mut matches = Box::pin(sink.sidecar.search(
                query,
                SearchTerms::Typed,
                &SessionFilter::default(),
                10,
            ));
            assert!(matches.next().await.is_none(), "sensitive content indexed: {query}");
        }
    }

    #[rstest]
    #[case(None, "Reasoned")]
    #[case(Some(185), "Reasoning · 185 tokens")]
    #[case(Some(0), "Reasoning · 0 tokens")]
    #[tokio::test]
    async fn reasoning_metadata_survives_storage_and_rendering(
        #[case] tokens: Option<u64>,
        #[case] label: &str,
    ) {
        let sink = Sink::new(mem_store().await, AiSessionDatabase::in_memory().await.unwrap());
        let mut msg = sample_message();
        msg.role = Role::Assistant;
        msg.content = vec![Content::ReasoningSummary { tokens }];
        sink.append(msg.clone()).await.unwrap();
        let rebuilt = AiSessionDatabase::in_memory().await.unwrap();
        sink.records.build(&rebuilt).await.unwrap();
        let mut messages = Box::pin(rebuilt.messages(&msg.session));
        let stored = messages.next().await.unwrap().unwrap();
        assert_eq!(stored.content, msg.content);
        let block = crate::grpc::ai::agent::pb::ContentBlock::from(stored.content[0].clone());
        assert_eq!(
            block.block,
            Some(crate::grpc::ai::agent::pb::content_block::Block::ReasoningSummary(
                crate::grpc::ai::agent::pb::ReasoningSummary { tokens }
            ))
        );
        let mut transcript = Box::pin(rebuilt.transcript(&msg.session));
        assert_eq!(transcript.next().await.unwrap().unwrap(), format!("assistant: {label}\n"));
    }

    /// A row of model call `turn` reporting `output` tokens, `reasoning` of them thinking.
    fn call_message(source: &str, turn: &str, output: u64, reasoning: Option<u64>) -> Message {
        let mut msg = sample_message();
        msg.id = RecordId(atuin_common::utils::uuid_v7());
        msg.source_id = source.to_owned().into();
        msg.role = Role::Assistant;
        msg.turn_id = Some(turn.to_owned());
        msg.usage = Some(Usage {
            output: Some(output),
            reasoning,
            ..Usage::default()
        });
        msg
    }

    /// Output and reasoning tokens a session is charged.
    async fn charged(db: &AiSessionDatabase) -> (u64, u64) {
        let usage = db.get_session(&sample_handle()).await.unwrap().unwrap().usage;
        (usage.output.unwrap(), usage.reasoning.unwrap())
    }

    /// A failed write on either side, a restart and a replay leave one call counted once,
    /// live and after a rebuild from the records.
    #[rstest]
    #[case(false)]
    #[case(true)]
    #[tokio::test]
    async fn usage_survives_failed_writes_and_restart(#[case] fail_projection: bool) {
        let dir = tempfile::tempdir().unwrap();
        let records_path = dir.path().join("records.db");
        let sidecar_path = dir.path().join("sessions.db");
        let store = SqliteStore::new(records_path.as_os_str(), NOP_STORE_TIMEOUT).await.unwrap();
        let records = AiSessionStore::builder()
            .store(store)
            .host_id(HostId(atuin_common::utils::uuid_v7()))
            .key(Key::generate())
            .build();
        let sidecar = AiSessionDatabase::open(&sidecar_path).await.unwrap();
        let sink = Sink::new(records.clone(), sidecar.clone());
        let mut tail = sink.tail.subscribe();
        let path = if fail_projection {
            &sidecar_path
        } else {
            &records_path
        };
        let fault =
            atuin_common::db::sqlite::Sqlite::builder(path.as_os_str()).open().await.unwrap();
        let sql = if fail_projection {
            "CREATE TRIGGER fail_write BEFORE INSERT ON messages BEGIN SELECT RAISE(FAIL, \
             'injected failure'); END"
        } else {
            "CREATE TRIGGER fail_write BEFORE INSERT ON store BEGIN SELECT RAISE(FAIL, 'injected \
             failure'); END"
        };
        atuin_common::db::query(sql).execute(fault.pool()).await.unwrap();
        let message = |source: &str| {
            let mut msg = call_message(source, "call", 999, Some(185));
            // Exercise compressed content too.
            msg.content = vec![Content::Text("hello ".repeat(100)), Content::ReasoningSummary {
                tokens: None,
            }];
            msg
        };
        assert!(sink.append(message("first")).await.is_err());
        atuin_common::db::query("DROP TRIGGER fail_write").execute(fault.pool()).await.unwrap();
        sink.append(message("second")).await.unwrap();
        let mut delivered = Vec::new();
        while let Ok(event) = tail.try_recv() {
            if let SessionTailEvent::Message(msg) = event {
                delivered.push(msg.source_id.to_string());
            }
        }
        let expected = if fail_projection {
            vec!["first", "second"]
        } else {
            vec!["second"]
        };
        assert_eq!(delivered, expected);
        drop(sink);
        // No in-memory dedup state survives this restart.
        let sink = Sink::new(records.clone(), sidecar.clone());
        sink.append(message("third")).await.unwrap();
        // A replay is also harmless.
        sink.append(message("second")).await.unwrap();
        assert_eq!(charged(&sidecar).await, (999, 185));
        let rebuilt = AiSessionDatabase::in_memory().await.unwrap();
        records.build(&rebuilt).await.unwrap();
        assert_eq!(charged(&rebuilt).await, (999, 185));
    }

    #[rstest]
    #[tokio::test]
    async fn incomplete_startup_recovery_disables_writers_until_repaired() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sessions.db");
        let records = mem_store().await;
        let sidecar = AiSessionDatabase::open(&path).await.unwrap();
        let mut msg = call_message("first", "call", 100, Some(42));
        // Simulate a crash after the record commit but before sidecar projection.
        records.push(&msg).await.unwrap();
        let fault =
            atuin_common::db::sqlite::Sqlite::builder(path.as_os_str()).open().await.unwrap();
        atuin_common::db::query(
            "CREATE TRIGGER fail_write BEFORE INSERT ON messages BEGIN SELECT RAISE(FAIL, \
             'injected failure'); END",
        )
        .execute(fault.pool())
        .await
        .unwrap();
        let capture = AiHarnessSessionCapture::open(
            records.clone(),
            sidecar.clone(),
            false,
            BlockingPool::new(NonZeroUsize::MIN),
        );
        assert!(!capture.ready().await);
        let mut import = Box::pin(capture.import(None));
        assert!(matches!(import.next().await.unwrap(), ImportProgress::Finished {
            imported: 0,
            ..
        }));
        assert!(import.next().await.is_none());
        drop(capture);
        atuin_common::db::query("DROP TRIGGER fail_write").execute(fault.pool()).await.unwrap();
        let capture = AiHarnessSessionCapture::open(
            records,
            sidecar.clone(),
            false,
            BlockingPool::new(NonZeroUsize::MIN),
        );
        assert!(capture.ready().await);
        assert_eq!(charged(&sidecar).await, (100, 42), "recovery projects the stranded record");
        msg.id = RecordId(atuin_common::utils::uuid_v7());
        msg.source_id = "later".to_owned().into();
        capture.sink.append(msg.clone()).await.unwrap();
        assert_eq!(charged(&sidecar).await, (100, 42));
    }

    #[rstest]
    #[case::ready(Some(StoreState::Ready))]
    #[case::failed(Some(StoreState::Unavailable))]
    #[case::aborted(None)]
    #[tokio::test]
    async fn recovery_finishes_however_it_ends(#[case] end: Option<StoreState>) {
        let (capture, state) = AiHarnessSessionCapture::with_state(StoreState::Recovering).await;
        let recovery = capture.recovery();
        let wait = Duration::from_millis(50);
        assert!(tokio::time::timeout(wait, recovery.finished()).await.is_err());

        match end {
            Some(end) => {
                state.send_replace(end);
            }
            None => drop(state),
        }
        assert!(tokio::time::timeout(wait, recovery.finished()).await.is_ok());
    }

    #[rstest]
    #[tokio::test]
    async fn ready_waits_for_background_recovery() {
        let records = mem_store().await;
        let sidecar = AiSessionDatabase::in_memory().await.unwrap();
        let msg = call_message("first", "call", 100, Some(42));
        records.push(&msg).await.unwrap();

        let capture = AiHarnessSessionCapture::open(
            records,
            sidecar.clone(),
            false,
            BlockingPool::new(NonZeroUsize::MIN),
        );
        // Capture and import hold on this, so the dedup gate sees every persisted message.
        assert!(capture.ready().await);
        assert!(sidecar.contains_message(&msg.session, &msg.source_id).await.unwrap());
    }

    /// A message of session `session`, from line `source`.
    fn message_of(session: &str, source: &str) -> Message {
        let mut msg = sample_message();
        msg.id = RecordId(atuin_common::utils::uuid_v7());
        msg.session.session = NativeSessionId::from(session.to_owned());
        msg.source_id = source.to_owned().into();
        msg
    }

    /// A rebuild (after a purge deleted records) takes out what the deleted records projected and
    /// replays the rest, reporting it is rebuilding meanwhile. Capture waits it out: its dedup gate
    /// would otherwise push a message the half-rebuilt sidecar has not got back yet a second time.
    #[rstest]
    #[tokio::test]
    async fn a_rebuild_replays_only_the_records_left_and_holds_capture() {
        let store = SqliteStore::in_memory(NOP_STORE_TIMEOUT).await.unwrap();
        let records = AiSessionStore::builder()
            .store(store.clone())
            .host_id(HostId(atuin_common::utils::uuid_v7()))
            .key(Key::generate())
            .build();
        let sidecar = AiSessionDatabase::in_memory().await.unwrap();
        let (kept, purged) = (message_of("kept", "k1"), message_of("purged", "p1"));
        records.push(&kept).await.unwrap();
        records.push(&purged).await.unwrap();
        let capture = AiHarnessSessionCapture::open(
            records,
            sidecar.clone(),
            false,
            BlockingPool::new(NonZeroUsize::MIN),
        );
        assert!(capture.ready().await);
        assert_eq!(capture.list_sessions(&SessionFilter::default()).await.unwrap().len(), 2);

        store.delete(purged.id).await.unwrap();
        // Keep the replay from starting, to look at the rebuild midway.
        let replay = sidecar.lock_reprojection().await;
        capture.rebuild().await.unwrap();
        assert!(capture.is_recovering(), "reads are told it is rebuilding");
        assert!(capture.list_sessions(&SessionFilter::default()).await.unwrap().is_empty());
        let again = tokio::spawn({
            let sink = capture.sink.clone();
            let kept = kept.clone();
            async move { sink.append(kept).await.unwrap() }
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!again.is_finished(), "capture waits for the rebuild");

        drop(replay);
        assert_eq!(again.await.unwrap(), Appended::Duplicate, "and then finds it projected");
        assert!(capture.ready().await);
        let listed: Vec<_> = capture
            .list_sessions(&SessionFilter::default())
            .await
            .unwrap()
            .into_iter()
            .map(|s| s.handle)
            .collect();
        assert_eq!(listed, std::slice::from_ref(&kept.session));
        let stored = store.all_tagged(&atuin_domain::record::RecordTag::AiSession).await.unwrap();
        assert_eq!(stored.len(), 1, "nothing was pushed twice");
    }

    /// A capture already past its wait for a rebuild, but still waiting for its locks when one
    /// empties the sidecar, waits the rebuild out too rather than check an empty sidecar.
    #[rstest]
    #[tokio::test]
    async fn a_capture_overtaken_by_a_rebuild_waits_for_it() {
        let store = SqliteStore::in_memory(NOP_STORE_TIMEOUT).await.unwrap();
        let records = AiSessionStore::builder()
            .store(store.clone())
            .host_id(HostId(atuin_common::utils::uuid_v7()))
            .key(Key::generate())
            .build();
        let sidecar = AiSessionDatabase::in_memory().await.unwrap();
        let kept = message_of("kept", "k1");
        records.push(&kept).await.unwrap();
        let capture = AiHarnessSessionCapture::open(
            records,
            sidecar.clone(),
            false,
            BlockingPool::new(NonZeroUsize::MIN),
        );
        assert!(capture.ready().await);

        let pending = capture.sink.pending_projection.lock().await;
        let again = tokio::spawn({
            let sink = capture.sink.clone();
            let kept = kept.clone();
            async move { sink.append(kept).await.unwrap() }
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        let replay = sidecar.lock_reprojection().await;
        capture.rebuild().await.unwrap();
        drop(pending);
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!again.is_finished(), "capture waits for the rebuild");

        drop(replay);
        assert_eq!(again.await.unwrap(), Appended::Duplicate);
        let stored = store.all_tagged(&atuin_domain::record::RecordTag::AiSession).await.unwrap();
        assert_eq!(stored.len(), 1, "nothing was pushed twice");
    }

    /// Asked during startup recovery, a rebuild resets the sidecar and leaves the replay to the
    /// recovery running, which starts over; without a store there is nothing to rebuild.
    #[rstest]
    #[tokio::test]
    async fn a_rebuild_during_recovery_leaves_it_the_replay() {
        let records = mem_store().await;
        let sidecar = AiSessionDatabase::in_memory().await.unwrap();
        let msg = message_of("kept", "k1");
        records.push(&msg).await.unwrap();
        let recovery = sidecar.lock_reprojection().await;
        let capture = AiHarnessSessionCapture::open(
            records,
            sidecar.clone(),
            false,
            BlockingPool::new(NonZeroUsize::MIN),
        );
        capture.rebuild().await.unwrap();
        assert!(capture.is_recovering());
        drop(recovery);
        assert!(capture.ready().await);
        assert!(sidecar.contains_message(&msg.session, &msg.source_id).await.unwrap());

        let nop = AiHarnessSessionCapture::nop().await;
        assert!(matches!(nop.rebuild().await, Err(RebuildError::Unavailable)));
    }

    /// Stops a replay between replaying and settling the store's state until released, telling
    /// the test it got there (see [`hooks::Point::ReplayBeforeSettle`]).
    #[derive(Debug)]
    pub(super) struct Gate {
        reached: tokio::sync::Semaphore,
        released: tokio::sync::Semaphore,
        /// Panic the next replay released from the gate, as a replay failing unexpectedly would.
        panic_next: AtomicBool,
    }

    impl Default for Gate {
        fn default() -> Self {
            Self {
                reached: tokio::sync::Semaphore::new(0),
                released: tokio::sync::Semaphore::new(0),
                panic_next: AtomicBool::new(false),
            }
        }
    }

    impl hooks::Hooks for Gate {
        fn at(&self, point: hooks::Point) -> futures::future::BoxFuture<'_, hooks::Fault> {
            Box::pin(async move {
                if point != hooks::Point::ReplayBeforeSettle {
                    return hooks::Fault::None;
                }
                self.reached.add_permits(1);
                self.released.acquire().await.unwrap().forget();
                if self.panic_next.swap(false, Ordering::SeqCst) {
                    hooks::Fault::Panic
                } else {
                    hooks::Fault::None
                }
            })
        }

        fn enter(&self, _: hooks::Section) -> Box<dyn Send> {
            Box::new(())
        }
    }

    impl Gate {
        /// Wait for a replay to have replayed, and hold it there.
        async fn reached(&self) {
            let reached = tokio::time::timeout(Duration::from_secs(10), self.reached.acquire());
            reached.await.expect("no replay reached the gate").unwrap().forget();
        }

        fn release(&self) {
            self.released.add_permits(1);
        }
    }

    /// A rebuild landing after a replay (startup recovery's, or an earlier rebuild's) has
    /// replayed, but before it settled the store's state, empties the sidecar under it: the
    /// replay must replay again rather than call the empty sidecar ready, which would miss every
    /// record in reads and let capture push them all a second time.
    #[rstest]
    #[tokio::test]
    async fn a_rebuild_before_a_replay_settles_is_replayed_before_ready(
        #[values(false, true)] during_a_rebuild: bool,
    ) {
        let store = SqliteStore::in_memory(NOP_STORE_TIMEOUT).await.unwrap();
        let records = AiSessionStore::builder()
            .store(store.clone())
            .host_id(HostId(atuin_common::utils::uuid_v7()))
            .key(Key::generate())
            .build();
        let sidecar = AiSessionDatabase::in_memory().await.unwrap();
        let msg = message_of("kept", "k1");
        records.push(&msg).await.unwrap();
        let gate = Arc::new(Gate::default());
        let capture = AiHarnessSessionCapture::open_with(
            records,
            sidecar.clone(),
            false,
            BlockingPool::new(NonZeroUsize::MIN),
            Some(gate.clone() as Arc<dyn hooks::Hooks>),
        );
        let projected = || sidecar.contains_message(&msg.session, &msg.source_id);

        gate.reached().await;
        if during_a_rebuild {
            // Recovery settles; a first rebuild's replay then replays, and stops at the gate.
            gate.release();
            assert!(capture.ready().await);
            capture.rebuild().await.unwrap();
            gate.reached().await;
        }
        assert!(projected().await.unwrap(), "the replay has replayed");
        assert!(capture.is_recovering());

        capture.rebuild().await.unwrap();
        assert!(!projected().await.unwrap(), "the rebuild emptied the sidecar");
        // The replay goes on to settle, sees the reset, and replays again.
        gate.release();
        gate.reached().await;
        assert!(capture.is_recovering(), "not ready before that replay settles");
        assert!(projected().await.unwrap(), "replayed after the reset");
        gate.release();
        assert!(capture.ready().await);
        assert!(projected().await.unwrap());

        // Capture trusts the sidecar again, rightly: nothing is pushed twice.
        assert_eq!(capture.sink.append(msg).await.unwrap(), Appended::Duplicate);
        let stored = store.all_tagged(&RecordTag::AiSession).await.unwrap();
        assert_eq!(stored.len(), 1);
    }

    /// A replay that panics leaves the store unavailable, not stuck recovering, and a later
    /// rebuild starts a replay of its own rather than wait on the one that panicked.
    #[rstest]
    #[tokio::test]
    async fn a_rebuild_after_a_replay_panicked_replays() {
        let records = mem_store().await;
        let sidecar = AiSessionDatabase::in_memory().await.unwrap();
        let msg = message_of("kept", "k1");
        records.push(&msg).await.unwrap();
        let gate = Arc::new(Gate::default());
        let capture = AiHarnessSessionCapture::open_with(
            records,
            sidecar.clone(),
            false,
            BlockingPool::new(NonZeroUsize::MIN),
            Some(gate.clone() as Arc<dyn hooks::Hooks>),
        );
        gate.reached().await;
        gate.release();
        assert!(capture.ready().await);

        // A rebuild's replay panics before settling.
        gate.panic_next.store(true, Ordering::SeqCst);
        capture.rebuild().await.unwrap();
        gate.reached().await;
        gate.release();
        let mut state = capture.state.clone();
        let unavailable = state.wait_for(|state| *state == StoreState::Unavailable);
        tokio::time::timeout(Duration::from_secs(10), unavailable)
            .await
            .expect("the panicked replay left the store recovering")
            .unwrap();

        // The next rebuild replays, and the store is ready with the record back.
        capture.rebuild().await.unwrap();
        gate.reached().await;
        gate.release();
        let ready = tokio::time::timeout(Duration::from_secs(10), capture.ready());
        assert!(ready.await.expect("the rebuild never ended"));
        assert!(sidecar.contains_message(&msg.session, &msg.source_id).await.unwrap());
    }

    /// A replay that panics after a rebuild emptied the sidecar under it neither ends that
    /// rebuild unavailable nor stalls it: the coordinator replays again, and the store ends ready.
    #[rstest]
    #[tokio::test]
    async fn a_replay_panicking_after_a_rebuild_replays_again() {
        let records = mem_store().await;
        let sidecar = AiSessionDatabase::in_memory().await.unwrap();
        let msg = message_of("kept", "k1");
        records.push(&msg).await.unwrap();
        let gate = Arc::new(Gate::default());
        let capture = AiHarnessSessionCapture::open_with(
            records,
            sidecar.clone(),
            false,
            BlockingPool::new(NonZeroUsize::MIN),
            Some(gate.clone() as Arc<dyn hooks::Hooks>),
        );
        // Startup recovery's replay has replayed; a rebuild empties the sidecar under it.
        gate.reached().await;
        capture.rebuild().await.unwrap();
        // Then it panics.
        gate.panic_next.store(true, Ordering::SeqCst);
        gate.release();
        // The replay after it holds at the gate: still recovering, not unavailable.
        gate.reached().await;
        assert!(capture.is_recovering());
        gate.release();
        let ready = tokio::time::timeout(Duration::from_secs(10), capture.ready());
        assert!(ready.await.expect("the rebuild never ended"));
        assert!(sidecar.contains_message(&msg.session, &msg.source_id).await.unwrap());
    }

    /// A rebuild whose caller gives up midway (a client disconnecting) still ends ready, rather
    /// than leave the store recovering with no replay to end it.
    #[rstest]
    #[tokio::test]
    async fn a_rebuild_given_up_on_still_ends_ready() {
        let records = mem_store().await;
        let sidecar = AiSessionDatabase::in_memory().await.unwrap();
        let msg = message_of("kept", "k1");
        records.push(&msg).await.unwrap();
        let capture = AiHarnessSessionCapture::open(
            records,
            sidecar.clone(),
            false,
            BlockingPool::new(NonZeroUsize::MIN),
        );
        assert!(capture.ready().await);

        // The rebuild waits for capture's lock, and is given up on there.
        let local = sidecar.lock_local_projection().await;
        let wait = Duration::from_millis(50);
        assert!(tokio::time::timeout(wait, capture.rebuild()).await.is_err());
        let mut state = capture.state.clone();
        state.wait_for(|state| *state == StoreState::Recovering).await.unwrap();
        drop(local);
        let ready = tokio::time::timeout(Duration::from_secs(10), capture.ready());
        assert!(ready.await.expect("the rebuild never ended"));
        assert!(sidecar.contains_message(&msg.session, &msg.source_id).await.unwrap());
    }

    /// A rebuild whose replay fails after emptying the sidecar leaves it missing persisted
    /// messages: capture must not trust its dedup gate then. A line already in the record store is
    /// refused rather than pushed again, and its transcript is not checkpointed past, until a
    /// rebuild succeeds -- which then finds it projected.
    #[rstest]
    #[tokio::test]
    async fn a_failed_rebuild_refuses_capture_until_a_rebuild_succeeds() {
        use atuin_common::harnesstools::session::{Checkpoint, SessionId};

        use super::engine::store as capture_rows;
        use super::message_enricher::MessageEnricher;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sessions.db");
        let store = SqliteStore::in_memory(NOP_STORE_TIMEOUT).await.unwrap();
        let records = AiSessionStore::builder()
            .store(store.clone())
            .host_id(HostId(atuin_common::utils::uuid_v7()))
            .key(Key::generate())
            .build();
        let sidecar = AiSessionDatabase::open(&path).await.unwrap();
        let msg = message_of("kept", "k1");
        records.push(&msg).await.unwrap();
        let capture = AiHarnessSessionCapture::open(
            records,
            sidecar.clone(),
            false,
            BlockingPool::new(NonZeroUsize::MIN),
        );
        assert!(capture.ready().await);
        let stored = async || store.all_tagged(&RecordTag::AiSession).await.unwrap().len();

        // The rebuild empties the sidecar, then its replay fails.
        let fault =
            atuin_common::db::sqlite::Sqlite::builder(path.as_os_str()).open().await.unwrap();
        atuin_common::db::query(
            "CREATE TRIGGER fail_write BEFORE INSERT ON messages BEGIN SELECT RAISE(FAIL, \
             'injected failure'); END",
        )
        .execute(fault.pool())
        .await
        .unwrap();
        capture.rebuild().await.unwrap();
        assert!(!capture.ready().await, "the replay failed");
        assert!(!sidecar.contains_message(&msg.session, &msg.source_id).await.unwrap());

        // Capture is refused, and pushes nothing.
        assert!(matches!(capture.sink.append(msg.clone()).await, Err(AppendError::Unavailable)));
        assert_eq!(stored().await, 1, "the persisted message was not pushed again");

        // The listener holds the line, unpushed and not checkpointed past.
        let session = SessionId::from("kept".to_owned());
        let enricher = MessageEnricher::new(HarnessKind::ClaudeCode);
        let handle = enricher.handle(&session);
        assert_eq!(handle, msg.session);
        let checkpoint = Checkpoint { at: 40, digest: 7 };
        let listener = tokio::spawn({
            let (sink, msg) = (capture.sink.clone(), msg.clone());
            async move {
                let mut stuck = std::collections::HashSet::new();
                capture_rows(&sink, &enricher, &mut stuck, &session, vec![msg], checkpoint).await;
                stuck
            }
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!listener.is_finished(), "capture pauses while the store is unavailable");
        assert_eq!(stored().await, 1);
        assert_eq!(sidecar.checkpoint(&handle).await.unwrap(), None, "no checkpoint past it");

        // A rebuild that succeeds makes the store ready again: the held line is captured as the
        // duplicate it is, and checkpointed past.
        atuin_common::db::query("DROP TRIGGER fail_write").execute(fault.pool()).await.unwrap();
        capture.rebuild().await.unwrap();
        assert!(capture.ready().await);
        let stuck = tokio::time::timeout(Duration::from_secs(10), listener).await;
        assert!(stuck.expect("capture never resumed").unwrap().is_empty());
        assert_eq!(sidecar.checkpoint(&handle).await.unwrap(), Some(checkpoint));
        assert_eq!(capture.sink.append(msg).await.unwrap(), Appended::Duplicate);
        assert_eq!(stored().await, 1, "nothing was pushed twice");
    }

    /// After a failed startup recovery, capture never started and the store stays unavailable
    /// until restart: a rebuild is refused.
    #[rstest]
    #[tokio::test]
    async fn a_failed_startup_recovery_refuses_rebuilds() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sessions.db");
        let records = mem_store().await;
        let sidecar = AiSessionDatabase::open(&path).await.unwrap();
        let msg = message_of("kept", "k1");
        records.push(&msg).await.unwrap();
        let fault =
            atuin_common::db::sqlite::Sqlite::builder(path.as_os_str()).open().await.unwrap();
        atuin_common::db::query(
            "CREATE TRIGGER fail_write BEFORE INSERT ON messages BEGIN SELECT RAISE(FAIL, \
             'injected failure'); END",
        )
        .execute(fault.pool())
        .await
        .unwrap();
        let capture = AiHarnessSessionCapture::open(
            records,
            sidecar,
            false,
            BlockingPool::new(NonZeroUsize::MIN),
        );
        assert!(!capture.ready().await);
        assert!(matches!(capture.sink.append(msg).await, Err(AppendError::Unavailable)));
        assert!(matches!(capture.rebuild().await, Err(RebuildError::Unavailable)));
        assert!(!capture.is_available());
    }

    /// Recovery replays only what the sidecar has not projected yet, and reports how far it got.
    #[rstest]
    #[tokio::test]
    async fn recovery_is_incremental_and_reports_progress() {
        let records = mem_store().await;
        let sidecar = AiSessionDatabase::in_memory().await.unwrap();
        for source in ["a", "b", "c"] {
            records.push(&call_message(source, source, 1, None)).await.unwrap();
        }
        let open = || {
            AiHarnessSessionCapture::open(
                records.clone(),
                sidecar.clone(),
                false,
                BlockingPool::new(NonZeroUsize::MIN),
            )
        };

        let first = open();
        assert!(first.ready().await);
        assert_eq!(first.recovery_progress(), (3, 3), "a fresh sidecar replays everything");
        drop(first);

        let again = open();
        assert!(again.ready().await);
        assert_eq!(again.recovery_progress(), (0, 0), "nothing new to replay");
    }

    #[rstest]
    #[tokio::test]
    async fn concurrent_rows_of_one_call_count_usage_once() {
        let records = mem_store().await;
        let sink = Sink::new(records.clone(), AiSessionDatabase::in_memory().await.unwrap());
        let first = call_message("first", "call", 100, Some(42));
        let second = call_message("second", "call", 100, Some(42));
        let (a, b) = tokio::join!(sink.append(first), sink.append(second));
        a.unwrap();
        b.unwrap();
        let rebuilt = AiSessionDatabase::in_memory().await.unwrap();
        records.build(&rebuilt).await.unwrap();
        assert_eq!(charged(&rebuilt).await, (100, 42));
    }

    /// Reasoning is usage: a call's thinking tokens count once at the most any of its rows
    /// reported (a thinking line can carry the call's opening usage, a later line the final
    /// count), however its rows interleave with another call's. Markers stay presence markers,
    /// labelled from their own row's usage.
    #[rstest]
    #[tokio::test]
    async fn split_and_interleaved_calls_count_reasoning_once() {
        let sink = Sink::new(mem_store().await, AiSessionDatabase::in_memory().await.unwrap());
        for (index, (turn, marker, output, reasoning)) in [
            ("a", true, 10, None),
            ("b", true, 50, Some(42)),
            ("a", false, 999, Some(185)),
            ("a", false, 999, Some(185)),
        ]
        .into_iter()
        .enumerate()
        {
            let mut msg = call_message(&format!("u{index}"), turn, output, reasoning);
            msg.content = if marker {
                vec![Content::ReasoningSummary { tokens: None }]
            } else {
                vec![Content::Text("hello".to_owned())]
            };
            sink.append(msg).await.unwrap();
        }
        assert_eq!(charged(&sink.sidecar).await, (1049, 227));
        let transcript: Vec<String> =
            sink.sidecar.transcript(&sample_handle()).map(Result::unwrap).collect().await;
        assert_eq!(transcript[..2], [
            "assistant: Reasoned\n".to_owned(),
            "assistant: Reasoning · 42 tokens\n".to_owned()
        ]);
    }

    #[rstest]
    fn adapters_keep_reasoning_presence_without_payloads() {
        use atuin_common::harnesstools::session::{AnyMessage, Message as _};
        let claude = AnyMessage::Ccode(
            serde_json::from_value(serde_json::json!({
                "type": "assistant", "message": {"role": "assistant",
                    "content": [{"type": "thinking", "thinking": "PRIVATE_REASONING"}],
                    "usage": {"output_tokens": 999}}
            }))
            .unwrap(),
        );
        let pi = AnyMessage::Pi(
            serde_json::from_value(serde_json::json!({
                "type": "message", "id": "p1", "message": {"role": "assistant",
                    "content": [{"type": "thinking", "thinking": "PRIVATE_REASONING"}]}
            }))
            .unwrap(),
        );
        let codex = AnyMessage::Codex(
            serde_json::from_value(serde_json::json!({
                "type": "response_item", "payload": {"type": "reasoning",
                    "summary": [{"type": "summary_text", "text": "PRIVATE_REASONING"}],
                    "encrypted_content": "PRIVATE_ENCRYPTED"}
            }))
            .unwrap(),
        );
        for m in [claude, pi, codex] {
            assert_eq!(m.content(), vec![Content::ReasoningSummary { tokens: None }]);
        }
    }

    #[rstest]
    #[case(Role::System)]
    #[case(Role::Tool)]
    #[case(Role::Other("custom".to_owned()))]
    fn non_conversation_text_is_omitted(#[case] role: Role) {
        let mut msg = sample_message();
        msg.role = role;
        sanitize_message(&mut msg);
        assert!(msg.content.is_empty());
    }

    /// Model-written summaries and failure reasons are conversation whatever the row's role.
    #[rstest]
    fn summaries_and_errors_survive_sanitize(
        #[values(Role::System, Role::Other("compact".to_owned()))] role: Role,
    ) {
        let mut msg = sample_message();
        msg.role = role;
        msg.content = vec![
            Content::Summary("earlier: AWS_SECRET_ACCESS_KEY=SUMMARYSECRET".to_owned()),
            Content::Error("overloaded".to_owned()),
        ];
        sanitize_message(&mut msg);
        assert_eq!(msg.content, vec![
            Content::Summary("earlier: AWS_SECRET_ACCESS_KEY=****".to_owned()),
            Content::Error("overloaded".to_owned()),
        ]);
    }

    #[rstest]
    #[tokio::test]
    async fn append_returns_new_then_duplicate() {
        let sink = Sink::new(mem_store().await, AiSessionDatabase::in_memory().await.unwrap());
        let msg = sample_message();

        assert_eq!(sink.append(msg.clone()).await.unwrap(), Appended::New);
        assert_eq!(sink.append(msg).await.unwrap(), Appended::Duplicate);
    }

    #[rstest]
    #[tokio::test]
    async fn append_without_subscriber_does_not_error() {
        let sink = Sink::new(mem_store().await, AiSessionDatabase::in_memory().await.unwrap());

        sink.append(sample_message()).await.unwrap();

        let session = sink.sidecar.get_session(&sample_handle()).await.unwrap().unwrap();
        assert_eq!(session.message_count, 1);
    }

    #[rstest]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_appends_of_one_message_write_a_single_record() {
        let raw = SqliteStore::in_memory(NOP_STORE_TIMEOUT).await.unwrap();
        let records = AiSessionStore::builder()
            .store(raw.clone())
            .host_id(HostId(atuin_common::utils::uuid_v7()))
            .key(Key::generate())
            .build();
        let sink = Arc::new(Sink::new(records, AiSessionDatabase::in_memory().await.unwrap()));

        let outcomes = futures::future::join_all((0..8).map(|_| {
            let sink = Arc::clone(&sink);
            let msg = sample_message();
            async move { sink.append(msg).await.unwrap() }
        }))
        .await;

        assert_eq!(outcomes.iter().filter(|a| matches!(a, Appended::New)).count(), 1);
        assert_eq!(raw.all_tagged(&RecordTag::AiSession).await.unwrap().len(), 1);
        assert_eq!(
            sink.sidecar.get_session(&sample_handle()).await.unwrap().unwrap().message_count,
            1
        );
    }
}

/// The capture pipeline end to end: harness lines through the enricher, the sink and the
/// sidecar, including usage accounting across sessions and daemon restarts.
#[cfg(test)]
mod pipeline_tests {
    use atuin_client::ai_session::HarnessSession;
    use atuin_common::harnesstools::session::{AnyMessage, SessionId};
    use rstest::{fixture, rstest};

    use super::engine::{Start, warm};
    use super::message_enricher::MessageEnricher;
    use super::*;

    #[fixture]
    async fn sink() -> Sink {
        let store = SqliteStore::in_memory(NOP_STORE_TIMEOUT).await.unwrap();
        let records = AiSessionStore::builder()
            .store(store)
            .host_id(HostId(atuin_common::utils::uuid_v7()))
            .key(Key::generate())
            .build();
        Sink::new(records, AiSessionDatabase::in_memory().await.unwrap())
    }

    fn ccode(raw: serde_json::Value) -> AnyMessage {
        AnyMessage::Ccode(serde_json::from_value(raw).unwrap())
    }

    fn codex(raw: serde_json::Value) -> AnyMessage {
        AnyMessage::Codex(serde_json::from_value(raw).unwrap())
    }

    fn pi(raw: serde_json::Value) -> AnyMessage {
        AnyMessage::Pi(serde_json::from_value(raw).unwrap())
    }

    fn sid(id: &str) -> SessionId {
        SessionId::from(id.to_owned())
    }

    /// A Claude Code assistant row of model call `turn`, reporting `output` tokens.
    fn cc_assistant(uuid: &str, turn: &str, output: u64, ts: &str) -> AnyMessage {
        ccode(serde_json::json!({
            "type": "assistant", "uuid": uuid, "sessionId": "s1", "timestamp": ts,
            "message": {"role": "assistant", "id": turn,
                "content": [{"type": "tool_use", "id": format!("t-{uuid}"), "name": "Bash", "input": {}}],
                "usage": {"input_tokens": 2, "output_tokens": output}},
        }))
    }

    fn cc_tool_result(uuid: &str, ts: &str) -> AnyMessage {
        ccode(serde_json::json!({
            "type": "user", "uuid": uuid, "sessionId": "s1", "timestamp": ts,
            "message": {"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "t", "content": "ok"}]},
        }))
    }

    /// Capture a whole transcript of `lines` the way import does.
    async fn capture_all(
        sink: &Sink,
        enricher: &mut MessageEnricher,
        session: &SessionId,
        lines: &[AnyMessage],
    ) -> Vec<Appended> {
        let mut out = Vec::new();
        for m in lines {
            for msg in enricher.capture(session, m) {
                out.push(sink.append(msg).await.unwrap());
            }
        }
        for msg in enricher.finish(session) {
            out.push(sink.append(msg).await.unwrap());
        }
        out
    }

    /// Capture each `(session, lines)` transcript with a fresh enricher.
    async fn capture_sessions(sink: &Sink, kind: HarnessKind, sessions: &[(&str, &[AnyMessage])]) {
        for (session, lines) in sessions {
            capture_all(sink, &mut MessageEnricher::new(kind), &sid(session), lines).await;
        }
    }

    /// What the engine does for a transcript resumed from its checkpoint after a restart.
    async fn resumed(sink: &Sink, kind: HarnessKind, session: &SessionId) -> MessageEnricher {
        let mut enricher = MessageEnricher::new(kind);
        warm(sink, &mut enricher, session, Start::Resumed).await;
        enricher
    }

    fn handle(kind: HarnessKind, session: &str) -> HarnessSession {
        HarnessSession {
            harness: kind,
            session: atuin_client::ai_session::NativeSessionId::from(session.to_owned()),
        }
    }

    async fn output_of(sink: &Sink, handle: &HarnessSession) -> u64 {
        sink.sidecar.get_session(handle).await.unwrap().unwrap().usage.output.unwrap()
    }

    /// Output tokens of every session, in the live sidecar and in one rebuilt from the synced
    /// records: the two must agree.
    async fn outputs(sink: &Sink) -> std::collections::BTreeMap<String, u64> {
        let collect = |sessions: Vec<Session>| {
            sessions
                .into_iter()
                .map(|s| (s.handle.session.to_string(), s.usage.output.unwrap()))
                .collect::<std::collections::BTreeMap<_, _>>()
        };
        let live = collect(sink.sidecar.list_sessions(&SessionFilter::default()).await.unwrap());
        let rebuilt = AiSessionDatabase::in_memory().await.unwrap();
        sink.records.build(&rebuilt).await.unwrap();
        assert_eq!(
            collect(rebuilt.list_sessions(&SessionFilter::default()).await.unwrap()),
            live,
            "rebuild agrees"
        );
        // So does a startup reprojection over the live sidecar, which replays what it holds.
        sink.records.reproject(&sink.sidecar).await.unwrap();
        assert_eq!(
            collect(sink.sidecar.list_sessions(&SessionFilter::default()).await.unwrap()),
            live,
            "replay agrees"
        );
        live
    }

    /// Claude Code writes the tool_use blocks of one response on separate lines with the user
    /// tool_result lines between them (see fixtures/ccode/session1.jsonl). A restart that
    /// resumes between two rows of one call still counts the call once.
    #[rstest]
    #[tokio::test]
    async fn restart_mid_call_counts_its_usage_once(#[future] sink: Sink) {
        let sink = sink.await;
        let session = sid("s1");
        let lines = [
            cc_assistant("a1", "msg_X", 152, "2026-09-18T10:00:00.000Z"),
            cc_tool_result("u1", "2026-09-18T10:00:01.000Z"),
            cc_assistant("a2", "msg_X", 152, "2026-09-18T10:00:02.000Z"),
        ];

        // Daemon runs, captures the first two lines, then restarts.
        let mut before = MessageEnricher::new(HarnessKind::ClaudeCode);
        capture_all(&sink, &mut before, &session, &lines[..2]).await;
        let mut after = resumed(&sink, HarnessKind::ClaudeCode, &session).await;
        capture_all(&sink, &mut after, &session, &lines[2..]).await;

        assert_eq!(outputs(&sink).await["s1"], 152, "one model call, counted once");
    }

    /// Rows of one call split by another call's rows (A, B, A) count A once.
    #[rstest]
    #[tokio::test]
    async fn interleaved_calls_count_usage_once(#[future] sink: Sink) {
        let sink = sink.await;
        capture_sessions(&sink, HarnessKind::ClaudeCode, &[("s1", &[
            cc_assistant("a1", "msg_A", 10, "2026-09-18T10:00:00.000Z"),
            cc_assistant("b1", "msg_B", 5, "2026-09-18T10:00:01.000Z"),
            cc_assistant("a2", "msg_A", 10, "2026-09-18T10:00:02.000Z"),
        ])])
        .await;
        assert_eq!(outputs(&sink).await["s1"], 15);
    }

    /// When the split rows of one call report growing usage (streamed snapshots), the largest
    /// counts, as ccusage keeps the largest duplicate (`should_replace_deduped_entry`). Each
    /// row still carries what it reported.
    #[rstest]
    #[tokio::test]
    async fn split_rows_count_the_largest_usage(#[future] sink: Sink) {
        let sink = sink.await;
        capture_sessions(&sink, HarnessKind::ClaudeCode, &[("s1", &[
            cc_assistant("a1", "msg_A", 25, "2026-09-18T10:00:00.000Z"),
            cc_assistant("a2", "msg_A", 250, "2026-09-18T10:00:01.000Z"),
        ])])
        .await;
        assert_eq!(outputs(&sink).await["s1"], 250);
        let rows: Vec<_> = sink
            .sidecar
            .messages(&handle(HarnessKind::ClaudeCode, "s1"))
            .map(|m| m.unwrap().usage.unwrap().output.unwrap())
            .collect()
            .await;
        assert_eq!(rows, vec![25, 250]);
    }

    /// Two Codex `token_usage_record` lines written in the same millisecond (rollout timestamps
    /// are ms precision) with different usage are distinct rows, each counted.
    #[rstest]
    #[tokio::test]
    async fn idless_lines_in_one_millisecond_keep_their_usage(#[future] sink: Sink) {
        let sink = sink.await;
        let usage = |response: &str, output: u64| {
            codex(serde_json::json!({
                "type": "token_usage_record", "timestamp": "2026-09-18T10:00:00.123Z",
                "payload": {"turn_id": "t1", "response_id": response,
                    "usage": {"input_tokens": 1, "output_tokens": output}},
            }))
        };
        let mut enricher = MessageEnricher::new(HarnessKind::Codex);
        let outcomes =
            capture_all(&sink, &mut enricher, &sid("s1"), &[usage("r1", 5), usage("r2", 7)]).await;
        assert_eq!(outcomes, vec![Appended::New, Appended::New]);
        assert_eq!(output_of(&sink, &enricher.handle(&sid("s1"))).await, 12);
    }

    /// Two id-less user prompts identical in every field (a Codex rollout replaying history in
    /// one burst) are two rows, and stay two rows across a re-read from the start and a
    /// restart resumed after them -- where a third identical line is a third row.
    #[rstest]
    #[tokio::test]
    async fn identical_idless_lines_stay_distinct_rows(#[future] sink: Sink) {
        let sink = sink.await;
        let session = sid("s1");
        let prompt = codex(serde_json::json!({
            "type": "response_item", "timestamp": "2026-09-18T10:00:00.123Z",
            "payload": {"type": "message", "role": "user",
                "content": [{"type": "input_text", "text": "continue"}]},
        }));
        let twice = [prompt.clone(), prompt.clone()];
        let count = async || {
            let handle = handle(HarnessKind::Codex, "s1");
            sink.sidecar.get_session(&handle).await.unwrap().unwrap().message_count
        };

        let mut first = MessageEnricher::new(HarnessKind::Codex);
        let outcomes = capture_all(&sink, &mut first, &session, &twice).await;
        assert_eq!(outcomes, vec![Appended::New, Appended::New]);
        let mut reread = MessageEnricher::new(HarnessKind::Codex);
        let outcomes = capture_all(&sink, &mut reread, &session, &twice).await;
        assert_eq!(outcomes, vec![Appended::Duplicate, Appended::Duplicate]);
        assert_eq!(count().await, 2);

        let mut after = resumed(&sink, HarnessKind::Codex, &session).await;
        let third = capture_all(&sink, &mut after, &session, &[prompt]).await;
        assert_eq!(third, vec![Appended::New]);
        assert_eq!(count().await, 3);
    }

    /// Which of a parent and its fork (or subagent replay) were captured, in what order.
    #[derive(Clone, Copy, Debug)]
    enum Captured {
        ParentFirst,
        ForkFirst,
        /// The parent's transcript is gone: the fork is the only record of the copied calls.
        ForkOnly,
    }

    impl Captured {
        /// Capture the two transcripts accordingly and return what each session is charged.
        async fn charge(
            self,
            sink: &Sink,
            kind: HarnessKind,
            parent: (&str, &[AnyMessage]),
            fork: (&str, &[AnyMessage]),
        ) -> std::collections::BTreeMap<String, u64> {
            let order = match self {
                Self::ParentFirst => vec![parent, fork],
                Self::ForkFirst => vec![fork, parent],
                Self::ForkOnly => vec![fork],
            };
            capture_sessions(sink, kind, &order).await;
            outputs(sink).await
        }

        /// The parent keeps the calls copied into the fork, which is charged only for its own
        /// -- unless the parent was never captured, when the copies count once, in the fork.
        fn expected(
            self,
            parent: (&str, u64),
            fork: (&str, u64),
        ) -> std::collections::BTreeMap<String, u64> {
            match self {
                Self::ParentFirst | Self::ForkFirst => {
                    [(parent.0.to_owned(), parent.1), (fork.0.to_owned(), fork.1)].into()
                }
                Self::ForkOnly => [(fork.0.to_owned(), parent.1 + fork.1)].into(),
            }
        }
    }

    /// Pi `forkFrom` / `createBranchedSession` copy every entry verbatim (same ids, same usage,
    /// same timestamps) into a new session file whose header names `parentSession`.
    #[rstest]
    #[tokio::test]
    async fn pi_fork_counts_copied_usage_once(
        #[future] sink: Sink,
        #[values(Captured::ParentFirst, Captured::ForkFirst, Captured::ForkOnly)]
        captured: Captured,
    ) {
        let sink = sink.await;
        let entry = |id: &str, ts: &str, response: &str, output: u64| {
            pi(serde_json::json!({
                "type": "message", "id": id, "timestamp": ts,
                "message": {"role": "assistant", "responseId": response,
                    "content": [{"type": "text", "text": "hi"}],
                    "usage": {"input": 100, "output": output}},
            }))
        };
        let copied = entry("m1", "2026-09-18T10:00:00.000Z", "resp_1", 10);
        let parent: &[AnyMessage] = &[
            pi(serde_json::json!({
                "type": "session", "id": "parent", "timestamp": "2026-09-18T10:00:00.000Z",
                "cwd": "/w",
            })),
            copied.clone(),
        ];
        let fork: &[AnyMessage] = &[
            pi(serde_json::json!({
                "type": "session", "id": "fork", "timestamp": "2026-09-18T11:00:00.000Z",
                "cwd": "/w", "parentSession": "/sessions/1700000000_parent.jsonl",
            })),
            copied,
            entry("m2", "2026-09-18T11:00:01.000Z", "resp_2", 3),
        ];
        let charged =
            captured.charge(&sink, HarnessKind::Pi, ("parent", parent), ("fork", fork)).await;
        assert_eq!(charged, captured.expected(("parent", 10), ("fork", 3)));
        let fork_row = sink.sidecar.get_session(&handle(HarnessKind::Pi, "fork")).await.unwrap();
        assert_eq!(
            fork_row.unwrap().parent.map(|p| p.session.to_string()).as_deref(),
            Some("parent"),
            "fork is linked to its parent session"
        );
    }

    /// Claude Code `/branch` / `--fork-session` copies the original session's lines (same uuid,
    /// message.id and usage, `sessionId` rewritten, origin in `forkedFrom`) into a new file;
    /// `/btw` side-question files replay parent lines the same way (ccusage #913), their lines
    /// naming the parent session.
    #[rstest]
    #[case::fork("new", serde_json::json!({"sessionId": "new",
        "forkedFrom": {"sessionId": "orig", "messageUuid": "u1"}}))]
    #[case::btw_replay("agent-aside", serde_json::json!({"sessionId": "orig", "isSidechain": true}))]
    #[tokio::test]
    async fn copied_claude_code_lines_count_usage_once(
        #[future] sink: Sink,
        #[case] copy: &str,
        #[case] extra: serde_json::Value,
        #[values(Captured::ParentFirst, Captured::ForkFirst, Captured::ForkOnly)]
        captured: Captured,
    ) {
        let sink = sink.await;
        let line = |uuid: &str, turn: &str, output: u64, extra: &serde_json::Value| {
            let mut raw = serde_json::json!({
                "type": "assistant", "uuid": uuid, "sessionId": "orig",
                "requestId": format!("req_{turn}"), "timestamp": "2026-09-23T22:41:00Z",
                "message": {"role": "assistant", "id": turn, "model": "claude-opus-5-5",
                    "content": [{"type": "text", "text": "hi"}],
                    "usage": {"input_tokens": 2, "output_tokens": output,
                        "cache_read_input_tokens": 1000, "cache_creation_input_tokens": 10}},
            });
            raw.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
            ccode(raw)
        };
        let orig: &[AnyMessage] = &[line("u1", "msg_A", 100, &serde_json::json!({}))];
        let copied: &[AnyMessage] =
            &[line("u1", "msg_A", 100, &extra), line("u2", "msg_B", 5, &extra)];
        let charged =
            captured.charge(&sink, HarnessKind::ClaudeCode, ("orig", orig), (copy, copied)).await;
        assert_eq!(charged, captured.expected(("orig", 100), (copy, 5)));
    }

    /// A forked Codex rollout copies the parent's rollout items (session_meta, messages,
    /// token_usage_record, ...) ahead of the child's own, stamped when copied (codex-rs
    /// `core/src/session/mod.rs`, `InitialHistory::Forked` + `ForkPersistence::Copied`).
    #[rstest]
    #[tokio::test]
    async fn codex_fork_counts_copied_usage_once(
        #[future] sink: Sink,
        #[values(Captured::ParentFirst, Captured::ForkFirst, Captured::ForkOnly)]
        captured: Captured,
    ) {
        let sink = sink.await;
        let usage_record = |ts: &str, response: &str, output: u64| {
            codex(serde_json::json!({
                "timestamp": ts, "type": "token_usage_record",
                "payload": {"turn_id": "t1", "response_id": response,
                    "usage": {"input_tokens": 10, "cached_input_tokens": 0, "output_tokens": output}},
            }))
        };
        let parent_meta = codex(serde_json::json!({
            "timestamp": "2026-09-18T10:00:00.000Z", "type": "session_meta",
            "payload": {"id": "parent", "cwd": "/work"},
        }));
        let parent_prompt = codex(serde_json::json!({
            "timestamp": "2026-09-18T10:00:00.001Z", "type": "response_item",
            "payload": {"type": "message", "id": "msg_p", "role": "user",
                "content": [{"type": "input_text", "text": "parent prompt"}]},
        }));
        let parent: &[AnyMessage] = &[
            parent_meta.clone(),
            parent_prompt.clone(),
            usage_record("2026-09-18T10:00:00.002Z", "resp_parent", 1_000),
        ];
        let child: &[AnyMessage] = &[
            codex(serde_json::json!({
                "timestamp": "2026-09-18T11:00:00.000Z", "type": "session_meta",
                "payload": {"id": "child", "forked_from_id": "parent", "cwd": "/work"},
            })),
            parent_meta,
            parent_prompt,
            usage_record("2026-09-18T11:00:00.002Z", "resp_parent", 1_000),
            // The child's own turn.
            codex(serde_json::json!({
                "timestamp": "2026-09-18T11:00:05.000Z", "type": "response_item",
                "payload": {"type": "message", "id": "msg_c", "role": "user",
                    "content": [{"type": "input_text", "text": "child prompt"}]},
            })),
            usage_record("2026-09-18T11:00:06.000Z", "resp_child", 7),
        ];
        let charged =
            captured.charge(&sink, HarnessKind::Codex, ("parent", parent), ("child", child)).await;
        assert_eq!(charged, captured.expected(("parent", 1_000), ("child", 7)));
    }

    /// Claude Code's compaction summary (`isCompactSummary`) is model-written conversation
    /// text: the parser emits it as `Content::Summary`, which capture policy keeps whatever the
    /// row's role (see `summaries_and_errors_survive_sanitize`).
    #[rstest]
    fn compact_summary_text_survives_capture() {
        let m = ccode(serde_json::json!({
            "type": "user", "uuid": "c1", "isCompactSummary": true,
            "timestamp": "2026-09-18T10:00:00.000Z",
            "message": {"role": "user", "content": "This session is being continued... Summary: X"},
        }));
        let mut msg =
            MessageEnricher::new(HarnessKind::ClaudeCode).capture(&sid("s1"), &m).pop().unwrap();
        sanitize_message(&mut msg);
        assert!(!msg.content.is_empty(), "summary text retained");
    }

    /// Execution payloads that Claude Code records as user text (`<local-command-stdout>`) must
    /// not be synced. The parser strips them; capture policy stays harness-agnostic.
    #[rstest]
    fn local_command_stdout_is_not_captured() {
        let m = ccode(serde_json::json!({
            "type": "user", "uuid": "c1", "timestamp": "2026-09-18T10:00:00.000Z",
            "message": {"role": "user", "content":
                "<local-command-stdout>PRIVATE_OUTPUT</local-command-stdout>"},
        }));
        let mut msg =
            MessageEnricher::new(HarnessKind::ClaudeCode).capture(&sid("s1"), &m).pop().unwrap();
        sanitize_message(&mut msg);
        assert!(
            !format!("{:?}", msg.content).contains("PRIVATE_OUTPUT"),
            "command output is an execution payload"
        );
    }

    /// A row that precedes every timestamped line (Claude Code `ai-title` et al.) takes the
    /// next timestamp, so an imported old session is not stamped as updated now.
    #[rstest]
    #[tokio::test]
    async fn untimed_first_row_takes_the_next_timestamp(#[future] sink: Sink) {
        let sink = sink.await;
        capture_sessions(&sink, HarnessKind::ClaudeCode, &[("s1", &[
            ccode(serde_json::json!({"type": "ai-title", "aiTitle": "Old work"})),
            cc_assistant("a1", "msg_A", 10, "2020-01-01T00:00:00.000Z"),
        ])])
        .await;
        let row = sink.sidecar.get_session(&handle(HarnessKind::ClaudeCode, "s1")).await;
        let row = row.unwrap().unwrap();
        assert_eq!(row.updated_at.year(), 2020, "updated_at = {}", row.updated_at);
        assert_eq!(row.started_at, row.updated_at);
        assert_eq!(row.title.as_deref(), Some("Old work"));
    }
}
