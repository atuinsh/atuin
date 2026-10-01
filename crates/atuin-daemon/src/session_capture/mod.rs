mod engine;
#[cfg(test)]
mod hooks;
mod import;
mod message_enricher;
mod recovery;

use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::Duration;

use atuin_client::ai_session::{
    AiSessionDatabase, AiSessionStore, Appended, BuildError, DbError, HarnessKind, HarnessSession,
    Message, PushError, ReprojectProgress, SearchTerms, Session, SessionFilter, SessionMatch,
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
use recovery::{Backoff, Coordinator, Msg};
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
    /// The append panicked, or the runtime is shutting down.
    #[error("the AI session capture stopped unexpectedly")]
    Aborted,
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

/// Capture's locks, as [`Sink::lock_ready`] takes them: the record being pushed or pushed but
/// not projected yet, and the sidecar's local projection lock. Owned, so a capture can hand them
/// to the task that finishes it (see [`Sink::append`]).
type CaptureLocks =
    (tokio::sync::OwnedMutexGuard<Option<Message>>, tokio::sync::OwnedMutexGuard<()>);

/// Cheap to clone: every clone shares the stores, the tail, the pending projection and the
/// state.
#[derive(Clone)]
pub(crate) struct Sink {
    records: AiSessionStore,
    sidecar: AiSessionDatabase,
    tail: broadcast::Sender<SessionTailEvent>,
    // Serialize the dedup gate + persistence across live capture and import. At most one record
    // can be waiting for projection (or, after a push that failed or panicked, for finding out
    // whether it was stored); repair it before admitting another capture.
    pending_projection: Arc<Mutex<Option<Message>>>,
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
            pending_projection: Arc::default(),
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

    /// The backoff a warm-up of a resumed session waits out between failed attempts (see
    /// [`engine::warm`]).
    #[cfg_attr(not(test), expect(clippy::unused_self))]
    fn warm_backoff(&self) -> Backoff {
        #[cfg(test)]
        if let Some(hooks) = &self.hooks {
            return hooks.warm_backoff();
        }
        Backoff::DEFAULT
    }

    pub(crate) fn subscribe(&self) -> BroadcastStream<SessionTailEvent> {
        BroadcastStream::new(self.tail.subscribe())
    }

    /// Persist and project `msg`, unless its logical message is already projected.
    ///
    /// Only while the store is [`StoreState::Ready`]: this waits out a rebuild, and refuses with
    /// [`AppendError::Unavailable`] (writing nothing) while the store is unavailable, as the
    /// sidecar the dedup gate trusts may then be missing persisted messages.
    ///
    /// Cancel-safe: dropped while it waits for the store or its locks, it has written nothing;
    /// once it has them, its dedup check, push and projection run to the end in a task of their
    /// own, which dropping this does not stop. (Stopped between its push and its projection, the
    /// record would be in the record store but neither projected nor pending, so the next
    /// capture of the line would find it missing from the sidecar and push it again.)
    pub(crate) async fn append(&self, mut msg: Message) -> Result<Appended, AppendError> {
        // Apply capture policy before either persistence path or the live tail. Keep structural
        // rows (even with no content) so parent links and usage accounting remain intact.
        sanitize_message(&mut msg);
        // Captured here, so on this host; a reproject reads the same from the record envelope.
        msg.host = Some(self.records.host_id());
        let locks = self.lock_ready().await?;
        let sink = self.clone();
        // The locks go with the task: released when it ends, however it ends (a panic unwinding
        // included, which leaves a tokio mutex usable).
        let section = tokio::spawn(async move { sink.append_locked(locks, msg).await });
        match section.await {
            Ok(appended) => appended,
            // A panic: what it may have pushed is pending (see `append_locked`). Cancelled: the
            // runtime is shutting down, and startup recovery projects whatever it pushed.
            Err(err) => {
                tracing::error!(?err, "an ai-session capture stopped unexpectedly");
                Err(AppendError::Aborted)
            }
        }
    }

    /// [`Self::append`] holding capture's locks, with the store found ready under them.
    async fn append_locked(
        self,
        (mut pending, local): CaptureLocks,
        msg: Message,
    ) -> Result<Appended, AppendError> {
        self.repair(&mut pending).await?;
        // Dedup gate: if this logical message is already projected it is already in the record
        // store too, so there is nothing to do. Stable source ids (see MessageEnricher::source_id)
        // make this reliable across re-captures and keep the record store free of duplicates.
        if self.sidecar.contains_message(&msg.session, &msg.source_id).await? {
            return Ok(Appended::Duplicate);
        }

        // Pending from before the push: should the push fail, or this task panic, once the
        // record may be stored, the next capture's repair finds out whether it was, and projects
        // it if so (see `repair`). Nothing else stops this task between its push and its
        // projection.
        *pending = Some(msg.clone());
        // Write the record store first: it is the synced source of truth and the sidecar is a
        // pure projection of it. If the sidecar write fails afterwards a later rebuild repairs it;
        // the reverse ordering could strand a message in the sidecar only -- lost on rebuild and
        // never synced.
        #[cfg(test)]
        if self.hook(hooks::Point::CapturePushing).await == hooks::Fault::Panic {
            std::panic::panic_any(hooks::INJECTED_PANIC);
        }
        self.records.push(&msg).await?;
        #[cfg(test)]
        if self.hook(hooks::Point::CapturePushed).await == hooks::Fault::Panic {
            std::panic::panic_any(hooks::INJECTED_PANIC);
        }
        let appended = self.project_and_broadcast(&msg).await?;
        *pending = None;
        drop(local);
        drop(pending);
        Ok(appended)
    }

    /// Take capture's locks with the store ready, so the sidecar holds every record and nothing
    /// empties it until they are released: what capture needs to trust the sidecar, for its
    /// dedup gate and for warming a resumed session's bookkeeping (see [`engine::warm`]).
    ///
    /// Waits out a rebuild (capture and import only start once startup recovery is over), and
    /// refuses with [`AppendError::Unavailable`] while the store is unavailable: a failed replay
    /// leaves a sidecar that may be missing persisted messages.
    async fn lock_ready(&self) -> Result<CaptureLocks, AppendError> {
        let mut state = self.state.clone();
        loop {
            if Self::settled(&mut state).await != StoreState::Ready {
                return Err(AppendError::Unavailable);
            }
            let pending = self.pending_projection.clone().lock_owned().await;
            // Keeps a reprojection of this host's records (after a sync) from projecting a
            // record capture pushes before capture does, and a rebuild from emptying the sidecar
            // between capture's check and its push. Taken after `pending`, never the other way
            // round.
            let local = self.sidecar.lock_local_projection().await;
            // Checked again under the lock: a rebuild says it is recovering before it takes the
            // lock to empty the sidecar, so one that began while this waited for the locks may
            // have emptied it already. Ready here, no rebuild can empty it until they are
            // released.
            let now = *state.borrow();
            match now {
                StoreState::Ready => return Ok((pending, local)),
                StoreState::Recovering => {}
                StoreState::Unavailable => return Err(AppendError::Unavailable),
            }
        }
    }

    /// Settle the record a capture was pushing when it failed or panicked, if any: projected if
    /// the record store holds it, dropped if not. The sidecar then holds every record again.
    async fn repair(&self, pending: &mut Option<Message>) -> Result<(), AppendError> {
        if let Some(previous) = pending.as_ref() {
            if self.records.holds(previous.id).await? {
                self.project_and_broadcast(previous).await?;
            }
            *pending = None;
        }
        Ok(())
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

/// The sync worker's handle on recovery: resolves once startup recovery is over, whether it
/// succeeded or not, and projects downloaded records beside capture.
#[derive(Debug, Clone)]
pub struct Recovery {
    state: watch::Receiver<StoreState>,
    /// The coordinator's mailbox, for a series only it may forget. Weak: the sync worker does
    /// not keep the coordinator alive. None without a persistent store.
    coordinator: Option<mpsc::WeakUnboundedSender<Msg>>,
}

impl Recovery {
    pub async fn finished(&self) {
        // A closed channel means the coordinator is gone: recovery is over either way.
        let _ = self.state.clone().wait_for(|state| *state != StoreState::Recovering).await;
    }

    /// Project what a sync downloaded into `sidecar`, beside capture
    /// ([`AiSessionStore::reproject_beside_capture`]). This never deletes a row of this host,
    /// which capture dedups against, and never sets the store's state: a series whose forgetting
    /// would is handed to the coordinator, which holds capture off first, as for a rebuild. A
    /// reprojection that kept being invalidated is left to the next sync.
    pub async fn project_synced(&self, records: &AiSessionStore, sidecar: &AiSessionDatabase) {
        let host = match records.reproject_beside_capture(sidecar).await {
            Ok(_) => return,
            Err(BuildError::ForgetHeldOff(host)) => host,
            Err(BuildError::Incomplete) => {
                tracing::warn!(
                    "synced ai-session records kept being invalidated while projected; the next \
                     sync projects the rest"
                );
                return;
            }
            Err(err) => {
                tracing::error!(?err, "failed to project synced ai-session records");
                return;
            }
        };
        let Some(coordinator) =
            self.coordinator.as_ref().and_then(mpsc::WeakUnboundedSender::upgrade)
        else {
            tracing::warn!(%host, "ai-session records rewritten, with nothing to replay them");
            return;
        };
        let (reply, answer) = oneshot::channel();
        if coordinator.send(Msg::SeriesRewritten { host, reply }).is_err() {
            return;
        }
        drop(coordinator);
        match answer.await {
            // Replaying, or (startup recovery failed) capture never started and nothing replays
            // until restart; or the coordinator is gone.
            Ok(Ok(()) | Err(RebuildError::Unavailable)) | Err(_) => {}
            // The series is found again by the next sync.
            Ok(Err(err)) => {
                tracing::error!(?err, %host, "failed to forget rewritten ai-session records");
            }
        }
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
    /// are replayed, and everything when the sidecar is fresh (a new install, or one deleted)
    /// or its watermarks were cleared (a migration that needs a backfill, a key change).
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
        // Tests wait out shorter backoffs.
        #[cfg(test)]
        let backoff = sink.hooks.as_ref().map_or(Backoff::DEFAULT, |hooks| hooks.backoff());
        #[cfg(not(test))]
        let backoff = Backoff::DEFAULT;
        let (coordinator, coordinating) =
            Coordinator::spawn(sink.clone(), state_tx, progress.clone(), backoff);

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
    /// replays again after this reset before the store is ready (see the `recovery` module).
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
            coordinator: self.coordinator.as_ref().map(mpsc::UnboundedSender::downgrade),
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
    struct Gate {
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

    /// Rewrite `host`'s ai-session records in `store`: each comes back at its index under a new
    /// record id, as when the series is reset and pushed again. A reprojection finds it rewritten
    /// under its watermark.
    async fn rewrite_series(store: &SqliteStore, key: &Key, host: HostId) {
        for record in store.all_tagged(&RecordTag::AiSession).await.unwrap() {
            if record.host.id != host {
                continue;
            }
            let rewritten = atuin_domain::record::Record {
                id: RecordId(atuin_common::utils::uuid_v7()),
                ..record.decrypt(key).unwrap()
            };
            store.delete(record.id).await.unwrap();
            store.push(&rewritten.encrypt(key)).await.unwrap();
        }
    }

    /// Watches a wipe: the store's state, and whether the sidecar still held a line, as the wipe
    /// takes capture's lock; then holds the coordinator after the wipe until released.
    #[derive(Debug)]
    struct HoldAfterWipe {
        watched: std::sync::OnceLock<(watch::Receiver<StoreState>, AiSessionDatabase, Message)>,
        /// What the wipe saw under capture's lock: the state, and whether the line was there.
        seen: parking_lot::Mutex<Vec<(StoreState, bool)>>,
        reached: tokio::sync::Semaphore,
        released: tokio::sync::Semaphore,
    }

    impl Default for HoldAfterWipe {
        fn default() -> Self {
            Self {
                watched: std::sync::OnceLock::new(),
                seen: parking_lot::Mutex::default(),
                reached: tokio::sync::Semaphore::new(0),
                released: tokio::sync::Semaphore::new(0),
            }
        }
    }

    impl hooks::Hooks for HoldAfterWipe {
        fn at(&self, point: hooks::Point) -> futures::future::BoxFuture<'_, hooks::Fault> {
            Box::pin(async move {
                match point {
                    hooks::Point::WipeLocked => {
                        if let Some((state, sidecar, msg)) = self.watched.get() {
                            let held = sidecar.contains_message(&msg.session, &msg.source_id).await;
                            self.seen.lock().push((*state.borrow(), held.unwrap()));
                        }
                    }
                    hooks::Point::AfterWipe => {
                        self.reached.add_permits(1);
                        self.released.acquire().await.unwrap().forget();
                    }
                    _ => {}
                }
                hooks::Fault::None
            })
        }
    }

    /// This host's record series rewritten under its watermark, found by the sync worker while
    /// the store is ready: what it projected is forgotten by the coordinator, as a rebuild. So
    /// capture is held off before anything is deleted, and a capture of a line already
    /// persisted waits for the replay, then finds it projected, rather than check the sidecar
    /// missing it and push it again.
    #[rstest]
    #[tokio::test]
    async fn a_local_series_rewritten_while_ready_holds_capture_off() {
        let store = SqliteStore::in_memory(NOP_STORE_TIMEOUT).await.unwrap();
        let (key, host) = (Key::generate(), HostId(atuin_common::utils::uuid_v7()));
        let records =
            AiSessionStore::builder().store(store.clone()).host_id(host).key(key.clone()).build();
        let sidecar = AiSessionDatabase::in_memory().await.unwrap();
        let msg = message_of("kept", "k1");
        records.push(&msg).await.unwrap();
        let hooks = Arc::new(HoldAfterWipe::default());
        let capture = AiHarnessSessionCapture::open_with(
            records.clone(),
            sidecar.clone(),
            false,
            BlockingPool::new(NonZeroUsize::MIN),
            Some(hooks.clone() as Arc<dyn hooks::Hooks>),
        );
        assert!(capture.ready().await);
        hooks.watched.set((capture.state.clone(), sidecar.clone(), msg.clone())).unwrap();
        let projected = || sidecar.contains_message(&msg.session, &msg.source_id);

        rewrite_series(&store, &key, host).await;
        let projector =
            crate::sync::spawn_ai_session_projector(records, sidecar.clone(), capture.recovery());
        projector.send(()).unwrap();
        let reached = tokio::time::timeout(Duration::from_secs(10), hooks.reached.acquire());
        reached.await.expect("the sync worker never had it forgotten").unwrap().forget();

        assert_eq!(
            *hooks.seen.lock(),
            [(StoreState::Recovering, true)],
            "recovering before anything was deleted"
        );
        assert!(capture.is_recovering());
        assert!(!projected().await.unwrap(), "forgotten");
        let again = tokio::spawn({
            let (sink, msg) = (capture.sink.clone(), msg.clone());
            async move { sink.append(msg).await.unwrap() }
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!again.is_finished(), "capture waits for the replay");

        hooks.released.add_permits(1);
        let again = tokio::time::timeout(Duration::from_secs(10), again).await;
        assert_eq!(again.expect("capture never resumed").unwrap(), Appended::Duplicate);
        assert!(capture.ready().await);
        assert!(projected().await.unwrap());
        let stored = store.all_tagged(&RecordTag::AiSession).await.unwrap();
        assert_eq!(stored.len(), 1, "nothing was pushed twice");
    }

    /// Counts the replays that gave up, invalidated pass after pass, and has the coordinator
    /// back off between them for a short while.
    #[derive(Debug, Default)]
    struct CountIncomplete {
        incomplete: std::sync::atomic::AtomicUsize,
    }

    impl hooks::Hooks for CountIncomplete {
        fn at(&self, point: hooks::Point) -> futures::future::BoxFuture<'_, hooks::Fault> {
            if point == hooks::Point::ReplayIncomplete {
                self.incomplete.fetch_add(1, Ordering::SeqCst);
            }
            Box::pin(async { hooks::Fault::None })
        }

        fn backoff(&self) -> recovery::Backoff {
            recovery::Backoff {
                first: Duration::from_millis(5),
                max: Duration::from_millis(40),
            }
        }
    }

    /// A replay whose reprojection keeps being invalidated, pass after pass, until it gives up is
    /// replayed again and again, backing off between them, for as long as the invalidations last:
    /// the store stays recovering meanwhile (never ready with the sidecar possibly missing
    /// records, nor unavailable, which at startup would last until restart), and capture waits.
    /// Once they stop, the store is ready.
    #[rstest]
    #[tokio::test]
    async fn an_invalidation_storm_keeps_the_store_recovering_until_it_stops(
        #[values(false, true)] at_startup: bool,
    ) {
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
        // Every watermark move fails, as when an invalidation lands in the middle of each pass.
        let fault =
            atuin_common::db::sqlite::Sqlite::builder(path.as_os_str()).open().await.unwrap();
        let invalidate = async || {
            atuin_common::db::query(
                "CREATE TRIGGER invalidate BEFORE INSERT ON reproject_watermark BEGIN SELECT \
                 RAISE(IGNORE); END",
            )
            .execute(fault.pool())
            .await
            .unwrap();
        };
        if at_startup {
            invalidate().await;
        }
        let hooks = Arc::new(CountIncomplete::default());
        let capture = AiHarnessSessionCapture::open_with(
            records,
            sidecar.clone(),
            false,
            BlockingPool::new(NonZeroUsize::MIN),
            Some(hooks.clone() as Arc<dyn hooks::Hooks>),
        );
        if !at_startup {
            assert!(capture.ready().await);
            invalidate().await;
            capture.rebuild().await.unwrap();
        }
        let again = tokio::spawn({
            let (sink, msg) = (capture.sink.clone(), msg.clone());
            async move { sink.append(msg).await.unwrap() }
        });

        // Recovering throughout the storm, however many replays it makes give up.
        let mut state = capture.state.clone();
        let storm = tokio::time::timeout(
            Duration::from_secs(10),
            state.wait_for(|state| *state != StoreState::Recovering),
        );
        let stormed = async {
            while hooks.incomplete.load(Ordering::SeqCst) < 5 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        };
        tokio::select! {
            settled = storm => panic!("settled during the storm: {:?}", settled.map(|s| *s.unwrap())),
            () = stormed => {}
        }
        assert!(capture.is_recovering());
        assert!(!again.is_finished(), "capture waits out the storm");

        atuin_common::db::query("DROP TRIGGER invalidate").execute(fault.pool()).await.unwrap();
        let ready = tokio::time::timeout(Duration::from_secs(10), capture.ready());
        assert!(ready.await.expect("never settled"), "ready once the storm is over");
        let again = tokio::time::timeout(Duration::from_secs(10), again).await;
        assert_eq!(again.expect("capture never resumed").unwrap(), Appended::Duplicate);
        let stored = store.all_tagged(&RecordTag::AiSession).await.unwrap();
        assert_eq!(stored.len(), 1, "nothing was pushed twice");
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

    /// A restored transcript writes a call captured without its input back as a note in its
    /// row's text. Re-captured, that row comes back under the source id already synced, with
    /// other content: it is a duplicate, never pushed again, and the synced row stands.
    #[rstest]
    #[tokio::test]
    async fn a_row_back_with_other_content_is_a_duplicate() {
        let raw = SqliteStore::in_memory(NOP_STORE_TIMEOUT).await.unwrap();
        let records = AiSessionStore::builder()
            .store(raw.clone())
            .host_id(HostId(atuin_common::utils::uuid_v7()))
            .key(Key::generate())
            .build();
        let sink = Sink::new(records, AiSessionDatabase::in_memory().await.unwrap());
        let mut synced = sample_message();
        synced.role = Role::Assistant;
        synced.content = vec![Content::ToolUse(ToolUse {
            id: ToolCallId::from("c1".to_owned()),
            name: "Bash".to_owned(),
            input: serde_json::json!({"command": "ls"}),
        })];
        let mut restored = synced.clone();
        restored.id = RecordId(atuin_common::utils::uuid_v7());
        restored.content = vec![Content::Text("Looking.\n\n[ran a shell command]".to_owned())];

        assert_eq!(sink.append(synced).await.unwrap(), Appended::New);
        assert_eq!(sink.append(restored).await.unwrap(), Appended::Duplicate);
        assert_eq!(raw.all_tagged(&RecordTag::AiSession).await.unwrap().len(), 1);
        let messages: Vec<Message> =
            sink.sidecar.messages(&sample_handle()).map(Result::unwrap).collect().await;
        assert_eq!(messages.len(), 1);
        assert!(
            matches!(messages[0].content.as_slice(), [Content::ToolUse(u)] if u.input.is_null())
        );
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

    /// Says when the future polled with it was woken.
    #[derive(Debug, Default)]
    struct Woken(AtomicBool);

    impl std::task::Wake for Woken {
        fn wake(self: Arc<Self>) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    /// A capture dropped once its push is stored (as the import RPC drops its stream, appends in
    /// flight and all, when its client disconnects) still projects its record: captured again,
    /// under a new record id as a re-read captures it, the line is a duplicate, stored once.
    ///
    /// The append is polled by hand, only once woken, and dropped as soon as a second handle on
    /// the record store sees its record, without being polled again. Were the dedup check, push
    /// and projection run in the caller's future, it would then be dropped between its push and
    /// its projection every time (the wake that tells the push is done comes once it is stored),
    /// leaving the record neither projected nor pending; the next capture of the line would push
    /// it again.
    #[rstest]
    #[tokio::test]
    async fn an_append_dropped_once_pushed_is_not_pushed_again() {
        use std::future::Future;
        use std::task::{Context, Poll, Waker};

        const ITERATIONS: usize = 50;
        let raw = SqliteStore::in_memory(NOP_STORE_TIMEOUT).await.unwrap();
        let records = AiSessionStore::builder()
            .store(raw.clone())
            .host_id(HostId(atuin_common::utils::uuid_v7()))
            .key(Key::generate())
            .build();
        let sink = Sink::new(records, AiSessionDatabase::in_memory().await.unwrap());
        let seen = raw.clone();

        let mut dropped = 0;
        for i in 0..ITERATIONS {
            let msg = message_of("dropped", &format!("m{i}"));
            let woken = Arc::new(Woken(AtomicBool::new(true)));
            let waker = Waker::from(woken.clone());
            let mut cx = Context::from_waker(&waker);
            let mut append = Box::pin(sink.append(msg.clone()));
            let mut ended = false;
            while !seen.contains(msg.id).await.unwrap() {
                if woken.0.swap(false, Ordering::SeqCst)
                    && let Poll::Ready(appended) = append.as_mut().poll(&mut cx)
                {
                    assert_eq!(appended.unwrap(), Appended::New);
                    ended = true;
                    break;
                }
                tokio::task::yield_now().await;
            }
            drop(append);
            dropped += usize::from(!ended);

            let mut again = msg.clone();
            again.id = RecordId(atuin_common::utils::uuid_v7());
            assert_eq!(sink.append(again).await.unwrap(), Appended::Duplicate, "iteration {i}");
            let stored = raw.all_tagged(&RecordTag::AiSession).await.unwrap();
            assert_eq!(stored.len(), i + 1, "iteration {i}: the line pushed twice");
        }
        assert!(dropped > ITERATIONS / 2, "only {dropped} appends dropped once pushed");
        let session = sink.sidecar.get_session(&message_of("dropped", "").session).await;
        assert_eq!(session.unwrap().unwrap().message_count, ITERATIONS as u64);
    }

    /// Panics a capture before its push, or once pushed before projecting, as the test says.
    #[derive(Debug)]
    struct PanicCapture(hooks::Point);

    impl hooks::Hooks for PanicCapture {
        fn at(&self, point: hooks::Point) -> futures::future::BoxFuture<'_, hooks::Fault> {
            let fault = if point == self.0 {
                hooks::Fault::Panic
            } else {
                hooks::Fault::None
            };
            Box::pin(async move { fault })
        }
    }

    /// A capture that panics leaves capture's locks usable and the line stored once. Its record
    /// is left pending either way: pushed, the next capture's repair projects it, and the line
    /// is then a duplicate; not pushed yet, the repair finds it missing from the record store and
    /// drops it, and the next capture pushes the line.
    #[rstest]
    #[case::before_its_push(hooks::Point::CapturePushing, Appended::New)]
    #[case::once_pushed(hooks::Point::CapturePushed, Appended::Duplicate)]
    #[tokio::test]
    async fn a_capture_that_panics_leaves_the_line_stored_once(
        #[case] at: hooks::Point,
        #[case] again: Appended,
    ) {
        let raw = SqliteStore::in_memory(NOP_STORE_TIMEOUT).await.unwrap();
        let records = AiSessionStore::builder()
            .store(raw.clone())
            .host_id(HostId(atuin_common::utils::uuid_v7()))
            .key(Key::generate())
            .build();
        let mut sink = Sink::new(records, AiSessionDatabase::in_memory().await.unwrap());
        let msg = message_of("panicked", "m");
        sink.hooks = Some(Arc::new(PanicCapture(at)));
        assert!(matches!(sink.append(msg.clone()).await, Err(AppendError::Aborted)));
        sink.hooks = None;

        let mut retry = msg.clone();
        retry.id = RecordId(atuin_common::utils::uuid_v7());
        let appended = tokio::time::timeout(Duration::from_secs(10), sink.append(retry));
        assert_eq!(appended.await.expect("capture's locks left held").unwrap(), again);
        assert_eq!(raw.all_tagged(&RecordTag::AiSession).await.unwrap().len(), 1);
        let row = sink.sidecar.get_session(&msg.session).await.unwrap().unwrap();
        assert_eq!(row.message_count, 1);
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

    /// An id-less Codex prompt: identical lines of it are told apart by their ordinal alone.
    fn continue_prompt() -> AnyMessage {
        codex(serde_json::json!({
            "type": "response_item", "timestamp": "2026-09-18T10:00:00.123Z",
            "payload": {"type": "message", "role": "user",
                "content": [{"type": "input_text", "text": "continue"}]},
        }))
    }

    /// A facade over a fresh in-memory store, ready, holding a Codex session `s1` of two
    /// identical id-less prompts (ordinals 0 and 1); and its record store.
    async fn two_prompts() -> (AiHarnessSessionCapture, SqliteStore) {
        two_prompts_with(None).await
    }

    /// [`two_prompts`], with test hooks.
    async fn two_prompts_with(
        hooks: Option<Arc<dyn hooks::Hooks>>,
    ) -> (AiHarnessSessionCapture, SqliteStore) {
        let store = SqliteStore::in_memory(NOP_STORE_TIMEOUT).await.unwrap();
        let records = AiSessionStore::builder()
            .store(store.clone())
            .host_id(HostId(atuin_common::utils::uuid_v7()))
            .key(Key::generate())
            .build();
        let sidecar = AiSessionDatabase::in_memory().await.unwrap();
        let capture = AiHarnessSessionCapture::open_with(
            records,
            sidecar,
            false,
            BlockingPool::new(NonZeroUsize::MIN),
            hooks,
        );
        assert!(capture.ready().await);
        let mut first = MessageEnricher::new(HarnessKind::Codex);
        let twice = [continue_prompt(), continue_prompt()];
        let outcomes = capture_all(&capture.sink, &mut first, &sid("s1"), &twice).await;
        assert_eq!(outcomes, vec![Appended::New, Appended::New]);
        (capture, store)
    }

    /// A third identical prompt, captured with `enricher` (warmed on resuming `s1`), is a third
    /// row: stored once, never dropped as a duplicate of the first two.
    async fn third_prompt_is_stored(
        capture: &AiHarnessSessionCapture,
        store: &SqliteStore,
        mut enricher: MessageEnricher,
    ) {
        let (session, lines) = (sid("s1"), [continue_prompt()]);
        let third = capture_all(&capture.sink, &mut enricher, &session, &lines);
        let third = tokio::time::timeout(Duration::from_secs(10), third).await;
        assert_eq!(third.expect("capture never resumed"), vec![Appended::New], "dropped");
        let handle = handle(HarnessKind::Codex, "s1");
        let row = capture.sink.sidecar.get_session(&handle).await.unwrap().unwrap();
        assert_eq!(row.message_count, 3);
        let stored = store.all_tagged(&atuin_domain::record::RecordTag::AiSession).await.unwrap();
        assert_eq!(stored.len(), 3, "each line pushed once");
    }

    /// A transcript resumed while a rebuild has emptied the sidecar and not replayed it yet: its
    /// warm-up waits for the replay, rather than read a sidecar missing the session's rows,
    /// which would count the identical prompts from ordinal 0 again and have the next one taken
    /// for a duplicate of the first, and dropped.
    ///
    /// The replay refills the sidecar without a new generation, so the generation a warm-up read
    /// at is the one capture finds under its lock once the store is ready: a check of the
    /// generation alone would not tell this warm-up was stale.
    #[rstest]
    #[tokio::test]
    async fn a_session_resumed_during_a_rebuild_is_warmed_once_replayed() {
        let (capture, store) = two_prompts().await;
        let replay = capture.sink.sidecar.lock_reprojection().await;
        capture.rebuild().await.unwrap();
        let wiped = capture.sink.sidecar.projection_generation().await.unwrap();
        let warming = tokio::spawn({
            let sink = capture.sink.clone();
            async move {
                let mut enricher = MessageEnricher::new(HarnessKind::Codex);
                warm(&sink, &mut enricher, &sid("s1"), Start::Resumed).await;
                enricher
            }
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        let early = warming.is_finished();

        drop(replay);
        let warmed = tokio::time::timeout(Duration::from_secs(10), warming).await;
        let enricher = warmed.expect("the warm-up never ended").unwrap();
        assert!(capture.ready().await);
        let generation = capture.sink.sidecar.projection_generation().await.unwrap();
        assert_eq!(generation, wiped, "the replay refilled the sidecar in the wipe's generation");
        third_prompt_is_stored(&capture, &store, enricher).await;
        assert!(!early, "the warm-up waits for the replay");
    }

    /// A wipe landing between a warm-up and the capture after it (the rebuild done by then, or
    /// still replaying while capture waits) leaves the warmed bookkeeping right: the replay
    /// restores every row it was warmed from.
    #[rstest]
    #[tokio::test]
    async fn a_wipe_between_warm_up_and_capture_keeps_the_ordinals(
        #[values(false, true)] replaying: bool,
    ) {
        let (capture, store) = two_prompts().await;
        let enricher = resumed(&capture.sink, HarnessKind::Codex, &sid("s1")).await;
        let replay = capture.sink.sidecar.lock_reprojection().await;
        capture.rebuild().await.unwrap();
        if replaying {
            let released = async {
                tokio::time::sleep(Duration::from_millis(50)).await;
                drop(replay);
            };
            tokio::join!(third_prompt_is_stored(&capture, &store, enricher), released);
        } else {
            drop(replay);
            assert!(capture.ready().await);
            third_prompt_is_stored(&capture, &store, enricher).await;
        }
    }

    /// Fails one step of a warm-up, the first time it runs.
    #[derive(Debug)]
    struct FailWarmStep {
        step: hooks::WarmStep,
        failed: std::sync::atomic::AtomicUsize,
    }

    impl hooks::Hooks for FailWarmStep {
        fn at(&self, point: hooks::Point) -> futures::future::BoxFuture<'_, hooks::Fault> {
            use std::sync::atomic::Ordering;

            let fault = if point == hooks::Point::WarmRead(self.step)
                && self.failed.fetch_add(1, Ordering::SeqCst) == 0
            {
                hooks::Fault::Fail
            } else {
                hooks::Fault::None
            };
            Box::pin(async move { fault })
        }

        fn warm_backoff(&self) -> Backoff {
            Backoff {
                first: Duration::from_millis(1),
                max: Duration::from_millis(10),
            }
        }
    }

    /// A warm-up whose repair or read fails once is retried, not taken from a partial view: a
    /// synthetic-id read taken for none would count the identical prompts from ordinal 0 again,
    /// and the third be taken for a duplicate of the first, and dropped. Warmed so, the
    /// bookkeeping is the same as a warm-up that never failed.
    #[rstest]
    #[tokio::test]
    async fn a_warm_up_whose_read_fails_is_retried(
        #[values(
            hooks::WarmStep::Repair,
            hooks::WarmStep::Session,
            hooks::WarmStep::Last,
            hooks::WarmStep::Synthetic,
            hooks::WarmStep::Titles
        )]
        step: hooks::WarmStep,
    ) {
        use std::sync::atomic::Ordering;

        let hooks = Arc::new(FailWarmStep {
            step,
            failed: std::sync::atomic::AtomicUsize::new(0),
        });
        let (capture, store) = two_prompts_with(Some(hooks.clone())).await;
        let session = sid("s1");
        let warming = resumed(&capture.sink, HarnessKind::Codex, &session);
        let mut retried = tokio::time::timeout(Duration::from_secs(10), warming)
            .await
            .expect("the warm-up never ended");
        let attempts = hooks.failed.load(Ordering::SeqCst);
        let mut clean = resumed(&capture.sink, HarnessKind::Codex, &session).await;

        let rows = retried.capture(&session, &continue_prompt());
        for row in rows.clone() {
            assert_eq!(capture.sink.append(row).await.unwrap(), Appended::New, "dropped");
        }
        let stored = store.all_tagged(&atuin_domain::record::RecordTag::AiSession).await.unwrap();
        assert_eq!(stored.len(), 3, "each line pushed once");
        let without_ids = |rows: Vec<Message>| -> Vec<Message> {
            rows.into_iter()
                .map(|mut row| {
                    row.id = atuin_domain::record::RecordId(uuid::Uuid::nil());
                    row
                })
                .collect()
        };
        let expected = clean.capture(&session, &continue_prompt());
        assert_eq!(without_ids(rows), without_ids(expected), "warmed the same");
        assert_eq!(attempts, 2, "failed once, then read again");
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

    /// A continuation's marker (`atuin ai resume --in`) is harness-injected text, which is not
    /// synced; the link it makes is, as every row's parent, so it survives sync and reprojection.
    #[rstest]
    fn a_continuation_marker_is_dropped_but_its_parent_is_kept() {
        use atuin_common::harnesstools::{AnyHarness, continuation};
        let marker = continuation::marker_text(AnyHarness::from_name("pi").unwrap(), "0199-orig");
        let m = ccode(serde_json::json!({
            "type": "user", "uuid": "c1", "isMeta": true, "timestamp": "2026-09-18T10:00:00.000Z",
            "message": {"role": "user", "content": marker},
        }));
        let mut msg =
            MessageEnricher::new(HarnessKind::ClaudeCode).capture(&sid("s1"), &m).pop().unwrap();
        sanitize_message(&mut msg);
        assert!(msg.content.is_empty(), "{:?}", msg.content);
        let parent = msg.parent.clone().expect("the marker names the parent");
        assert_eq!(parent.harness, HarnessKind::Pi);
        assert_eq!(parent.session.as_ref(), "0199-orig");
        let record = atuin_client::ai_session::AiSessionRecord::Message(msg).serialize();
        let back = atuin_client::ai_session::AiSessionRecord::deserialize(&record).unwrap();
        assert!(format!("{back:?}").contains("0199-orig"), "the record carries the parent");
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

    /// Serializes the tests that point `CODEX_HOME` somewhere: the variable is process-wide, and
    /// a rehydrate that found it unset would write to the real `~/.codex`.
    static CODEX_HOME: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    /// Write `session` out as Codex would find it, under a temporary `CODEX_HOME`.
    async fn rehydrate_codex(
        session: &atuin_common::harnesstools::rehydrate::RehydrateSession,
        home: &std::path::Path,
    ) -> std::path::PathBuf {
        let _env = CODEX_HOME.lock().await;
        // SAFETY: no other thread of these tests reads or writes the environment meanwhile.
        unsafe { std::env::set_var("CODEX_HOME", home) };
        let written = atuin_common::harnesstools::codex::rehydrate::rehydrate(session).await;
        // SAFETY: as above.
        unsafe { std::env::remove_var("CODEX_HOME") };
        let path = written.unwrap();
        assert!(path.starts_with(home), "{} is outside the test's CODEX_HOME", path.display());
        path
    }

    /// A Codex rollout's lines, read as capture reads them.
    async fn codex_rollout(id: &str, path: std::path::PathBuf) -> Vec<AnyMessage> {
        use atuin_common::harnesstools::codex::session::CodexSession;
        use atuin_common::harnesstools::session::Session as _;
        use futures::TryStreamExt;
        CodexSession::open(sid(id), path, BlockingPool::new(NonZeroUsize::MIN))
            .read()
            .map_ok(AnyMessage::from)
            .try_collect()
            .await
            .unwrap()
    }

    /// A Codex session written back out from its synced rows (rehydrated, to be resumed on
    /// another machine) and captured again there pushes no record: every line resolves to a row
    /// already synced.
    #[rstest]
    #[case::legacy_forked_subagent("legacy-forked-subagent.jsonl")]
    #[case::paginated_with_compaction("paginated-compacted.jsonl")]
    #[case::custom_tools_and_records("session1.jsonl")]
    #[case::legacy_without_ids_or_timestamps("legacy-bare.jsonl")]
    #[tokio::test]
    async fn a_rehydrated_codex_session_recaptures_as_nothing_new(
        #[future] sink: Sink,
        #[case] name: &str,
    ) {
        let sink = sink.await;
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../atuin-common/tests/fixtures/codex")
            .join(name);
        let first: serde_json::Value = serde_json::from_str(
            std::fs::read_to_string(&fixture).unwrap().lines().next().unwrap(),
        )
        .unwrap();
        let id = first["payload"]["id"].as_str().or(first["id"].as_str()).unwrap().to_owned();

        let lines = codex_rollout(&id, fixture).await;
        let mut enricher = MessageEnricher::new(HarnessKind::Codex);
        let captured = capture_all(&sink, &mut enricher, &sid(&id), &lines).await;
        assert!(captured.contains(&Appended::New));

        let handle = handle(HarnessKind::Codex, &id);
        let session = sink
            .sidecar
            .rehydrate_session(&handle, std::path::PathBuf::from("/elsewhere"))
            .await
            .unwrap()
            .unwrap();
        let home = tempfile::tempdir().unwrap();
        let path = rehydrate_codex(&session, home.path()).await;

        // Written in Codex's paginated history mode.
        let text = std::fs::read_to_string(&path).unwrap();
        let header: serde_json::Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
        assert_eq!(header["payload"]["history_mode"], "paginated");
        let again = codex_rollout(&id, path).await;
        let mut enricher = MessageEnricher::new(HarnessKind::Codex);
        let outcomes = capture_all(&sink, &mut enricher, &sid(&id), &again).await;
        let new = outcomes.iter().filter(|o| **o == Appended::New).count();
        assert_eq!(new, 0, "re-capturing the rehydrated rollout pushed {new} records");
    }
}
