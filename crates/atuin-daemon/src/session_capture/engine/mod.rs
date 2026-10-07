use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use atuin_client::ai_session::{
    Appended, DbError, HarnessKind, HarnessSession, Message, NativeSessionId, Session, SourceId,
};
use atuin_common::harnesstools::AnyHarness;
use atuin_common::harnesstools::session::{
    AnyMessage, CaptureError, Checkpoint, RuntimeError, SessionEvent, SessionId, TitleChange,
};
use atuin_common::sync::BlockingPool;
use futures::{Stream, StreamExt};
use parking_lot::Mutex;
use tokio::task::JoinHandle;
use tokio::time::Instant;

#[cfg(test)]
use super::hooks::WarmStep;
use super::message_enricher::{MessageEnricher, SYNTHETIC};
use super::{AppendError, CaptureLocks, Sink, StoreState};

/// Backoff bounds for retrying a harness listener whose session directory does not exist yet.
const LISTENER_RETRY_START: Duration = Duration::from_secs(2);
const LISTENER_RETRY_MAX: Duration = Duration::from_secs(60);

/// How long a live session with rows waiting for a timestamp may stay quiet before they are
/// stored with capture time: a transcript of untimed lines alone (Claude Code title lines)
/// would otherwise hold them until the daemon stops, and never checkpoint past them.
const UNTIMED_GRACE: Duration = Duration::from_secs(5);

pub struct SessionCaptureEngine {
    listeners: Vec<JoinHandle<()>>,
}

impl SessionCaptureEngine {
    pub fn spawn(sink: &Arc<Sink>, pool: &BlockingPool) -> Self {
        let mut listeners = Vec::new();

        for harness in AnyHarness::all() {
            let Some(sessions) = harness.sessions(pool) else {
                continue;
            };
            let kind = HarnessKind::from(harness);
            let sink = sink.clone();

            listeners.push(tokio::spawn(async move {
                // A harness whose session directory does not exist yet (not installed, or never
                // run) must not be dropped for the life of the daemon: retry with capped backoff
                // until it appears, so capture starts without a restart.
                let mut delay = LISTENER_RETRY_START;
                let listener = loop {
                    match sessions.listener() {
                        Ok(listener) => break listener,
                        Err(RuntimeError::NotFound(_)) => {
                            tokio::time::sleep(delay).await;
                            delay = (delay * 2).min(LISTENER_RETRY_MAX);
                        }
                        Err(e) => {
                            tracing::warn!(
                                ?e,
                                ?kind,
                                "ai-session listener failed; capture disabled for this harness"
                            );
                            return;
                        }
                    }
                };

                // Sessions `events()` has just (re)opened, with the checkpoint each was handed:
                // set before the new read yields anything, so its first event finds it here.
                let opened: Opened = Arc::default();
                let checkpoint = {
                    let (sink, opened) = (sink.clone(), opened.clone());
                    move |id: &SessionId| {
                        let (sink, opened, id) = (sink.clone(), opened.clone(), id.clone());
                        async move {
                            let from = checkpoint_of(&sink, kind, &id).await;
                            opened.lock().insert(id, from);
                            from
                        }
                    }
                };
                capture(kind, &sink, listener.events(checkpoint), &opened, UNTIMED_GRACE).await;
            }));
        }

        Self { listeners }
    }
}

/// Capture every event of one harness's listener until it ends. Rows waiting for a timestamp
/// are stored once their session has been quiet for `grace` (see [`UNTIMED_GRACE`]).
async fn capture(
    kind: HarnessKind,
    sink: &Sink,
    events: impl Stream<Item = Result<SessionEvent<AnyMessage>, CaptureError>>,
    opened: &Opened,
    grace: Duration,
) {
    futures::pin_mut!(events);
    let mut enricher = MessageEnricher::new(kind).keeping_patches(sink.keeps_patches());
    // Sessions with a failed append: their checkpoint must not move past the line that was
    // lost, or a restart would never re-read it.
    let mut stuck: HashSet<SessionId> = HashSet::new();
    // Rows of a transcript's content before it was replaced whose append failed: no read can
    // bring them back, so they are kept, and retried, and the checkpoint held, until stored.
    let mut held: Held = HashMap::new();
    // Sessions holding rows that wait for a timestamp: when they may be flushed, and the
    // checkpoint past the last line seen.
    let mut untimed: HashMap<SessionId, (Instant, Checkpoint)> = HashMap::new();
    // Which read of its transcript each session's stream is in ([`Checkpoint::generation`]):
    // one that changes under the same stream started over from the beginning of a transcript
    // replaced or cut short under it, which the line reader says, as positions can't (the new
    // content's first line may end past where the old content was read to).
    // ponytail: kept for every session seen, as `events()` keeps its handles.
    let mut reads: HashMap<SessionId, u32> = HashMap::new();

    loop {
        let due = untimed.values().map(|(at, _)| *at).min();
        tokio::select! {
            ev = events.next() => {
                let Some(ev) = ev else { break };
                let SessionEvent { session, checkpoint, message } = match ev {
                    Ok(ev) => ev,
                    Err(e) => {
                        tracing::warn!(?e, "capture error");
                        continue;
                    }
                };
                let opened = opened.lock().remove(&session);
                let read = reads.insert(session.clone(), checkpoint.generation);
                if let Some(from) = opened {
                    let start = Start::of(from, checkpoint);
                    // A new read retries whatever a failed append lost.
                    stuck.remove(&session);
                    untimed.remove(&session);
                    warm(sink, &mut enricher, &session, start).await;
                } else if read.is_some_and(|read| read != checkpoint.generation) {
                    restarted(sink, &mut enricher, &mut stuck, &mut held, &mut untimed, &session)
                        .await;
                }
                let rows = enricher.capture(&session, &message);
                if rows.is_empty() {
                    if enricher.has_untimed(&session) {
                        untimed.insert(session, (Instant::now() + grace, checkpoint));
                    }
                    continue;
                }
                untimed.remove(&session);
                store(sink, &enricher, &mut stuck, &mut held, &session, rows, checkpoint).await;
            }
            () = tokio::time::sleep_until(due.unwrap_or_else(Instant::now)), if due.is_some() => {
                let now = Instant::now();
                let quiet: Vec<SessionId> =
                    untimed.iter().filter(|(_, (at, _))| *at <= now).map(|(s, _)| s.clone()).collect();
                for session in quiet {
                    let Some((_, checkpoint)) = untimed.remove(&session) else { continue };
                    let rows = enricher.finish(&session);
                    store(sink, &enricher, &mut stuck, &mut held, &session, rows, checkpoint).await;
                }
            }
        }
    }
}

/// The read of `session` started over from the beginning of its transcript, which was replaced
/// in place (`atuin ai resume` switching it to another branch writes a new file over it) or cut
/// short: the line reader reads a file whose identity changed, or that shrank below what it had
/// read, again from its first byte, under the same stream, and says so on every line it reads
/// after ([`Checkpoint::generation`]).
///
/// Its bookkeeping is set up as for a read from the beginning: every line replays, so ordinals
/// of the lines keyed on their content count from 0 again, and those already stored resolve to
/// the rows they made (deduplicated by source id) instead of new rows under the next ordinals;
/// and no line of the new content follows a row of the old one. Rows the old content left
/// waiting for a timestamp are stored first with capture time, as a session gone quiet's are
/// ([`MessageEnricher::finish`]); their checkpoint is not, as it named the old content.
///
/// The old content is gone, so no read brings back a row of it whose append fails: such a row is
/// kept, as a failed append holds its line, and the session's checkpoint stays where it is until
/// a later append of the session stores it ([`Held`]). An append of the old content that failed
/// before the restart (its row, not kept, no read can bring back either) no longer holds the
/// checkpoint back: the new read is what a restart would read again.
async fn restarted(
    sink: &Sink,
    enricher: &mut MessageEnricher,
    stuck: &mut HashSet<SessionId>,
    held: &mut Held,
    untimed: &mut HashMap<SessionId, (Instant, Checkpoint)>,
    session: &SessionId,
) {
    tracing::debug!(%session, "ai-session transcript replaced or cut short; reading it again");
    untimed.remove(session);
    let rows = enricher.finish(session);
    hold(sink, held, session, rows).await;
    stuck.remove(session);
    warm(sink, enricher, session, Start::Beginning).await;
}

/// Each session's rows that no read of its transcript can bring back (see [`restarted`]) whose
/// append failed: kept, in order, until an append of them succeeds. While a session has any, its
/// checkpoint doesn't move.
pub(super) type Held = HashMap<SessionId, Vec<Message>>;

/// Append `rows` of `session` after those it holds already ([`Held`]), retrying those first;
/// whichever fails is held (again).
async fn hold(sink: &Sink, held: &mut Held, session: &SessionId, rows: Vec<Message>) {
    let pending = held.remove(session).unwrap_or_default();
    let mut failed = Vec::new();
    for msg in pending.into_iter().chain(rows) {
        if let Err(e) = append(sink, msg.clone()).await {
            tracing::warn!(?e, %session, "failed to capture ai-session message; holding it");
            failed.push(msg);
        }
    }
    if !failed.is_empty() {
        held.insert(session.clone(), failed);
    }
}

/// Append a session's rows, then checkpoint just past the line that completed them unless an
/// append failed, or the session holds rows no read can bring back ([`Held`]), which are retried
/// first.
///
/// A row refused because the store is unavailable (see [`Sink::append`]) pauses the listener
/// until the store is ready again (a rebuild succeeding), then is retried, so the line is neither
/// lost nor checkpointed past meanwhile. Should the store stay unavailable, a restart re-reads it
/// from the checkpoint.
pub(super) async fn store(
    sink: &Sink,
    enricher: &MessageEnricher,
    stuck: &mut HashSet<SessionId>,
    held: &mut Held,
    session: &SessionId,
    rows: Vec<Message>,
    checkpoint: Checkpoint,
) {
    if held.contains_key(session) {
        hold(sink, held, session, Vec::new()).await;
    }
    for msg in rows {
        if let Err(e) = append(sink, msg).await {
            tracing::warn!(?e, "failed to capture ai-session message");
            stuck.insert(session.clone());
        }
    }
    if stuck.contains(session) || held.contains_key(session) {
        return;
    }
    // ponytail: one checkpoint write per row; batch per session on idle if it shows up in
    // profiles.
    if let Err(e) = sink.sidecar.set_checkpoint(&enricher.handle(session), checkpoint).await {
        tracing::warn!(?e, "failed to record ai-session checkpoint");
    }
}

/// Append `msg`, waiting (not retrying hot) for the store to be ready whenever it is refused as
/// unavailable. Gives up only if the store's state can no longer change (the facade is gone).
pub(super) async fn append(sink: &Sink, msg: Message) -> Result<Appended, AppendError> {
    let mut state = sink.state.clone();
    loop {
        match sink.append(msg.clone()).await {
            Err(AppendError::Unavailable) => {
                tracing::debug!("ai-session store unavailable; capture paused until it is ready");
                if state.wait_for(|state| *state == StoreState::Ready).await.is_err() {
                    return Err(AppendError::Unavailable);
                }
            }
            appended => return appended,
        }
    }
}

/// Where a fresh read of a session starts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Start {
    Beginning,
    /// Just past the checkpoint it was handed, which still named its item.
    Resumed,
}

impl Start {
    /// Where a read handed `from` started, told by the checkpoint of the first event it yields.
    ///
    /// A session reads on from `from` only while `from` still names its item, and otherwise from
    /// its beginning; it does not say which. Positions only grow within one read -- a line's end
    /// offset past the line before it, a row's `seq` past the row before it -- so an event of a
    /// resumed read always lies strictly past `from`. A first event at or before `from` is
    /// therefore the beginning, as is any read handed no checkpoint at all.
    ///
    /// The converse is not exact from positions alone: a source rewritten so that its first item
    /// now ends past `from` (a first line longer than the old checkpoint's offset; an opencode
    /// aggregate recreated whose first row with a message has a greater `seq`) is read from its
    /// beginning but would be taken for a resume. A transcript's reader says when it started
    /// over ([`Checkpoint::generation`]: it found `from` no longer naming its line, or the file
    /// replaced or cut short under it), so only opencode's is left to the positions. That errs
    /// on the safe side: the bookkeeping is then warmed from rows of the old content, so a
    /// re-read id-less line that matches one of them is stored again under the next ordinal
    /// rather than deduplicated. The other mistake, a resume taken for the beginning, would
    /// reset the ordinals and silently drop genuinely new repeats of earlier lines, and cannot
    /// happen: a reader only says it started over when it did.
    pub(super) fn of(from: Option<Checkpoint>, first: Checkpoint) -> Self {
        match from {
            Some(from) if first.generation == 0 && first.at > from.at => Self::Resumed,
            _ => Self::Beginning,
        }
    }
}

/// The checkpoint handed to each session `events()` has (re)opened and not yielded from since.
type Opened = Arc<Mutex<HashMap<SessionId, Option<Checkpoint>>>>;

/// Set a session's bookkeeping up for a fresh read of its transcript: empty for one read from
/// the beginning, whose every line replays; warmed from the sidecar for one resumed past its
/// start, which replays none of the lines before it.
///
/// The sidecar is read as capture's dedup gate reads it: with the store ready, under capture's
/// locks (see [`Sink::lock_ready`]), waiting out a rebuild or a forget of this host's rows, and
/// pausing while the store is unavailable. Read while a wipe had emptied it and no replay had
/// refilled it yet, the bookkeeping would be warmed from a sidecar missing the session's rows:
/// identical id-less lines would count from ordinal 0 again, so a genuinely new one would take
/// the id of one stored before and be dropped as its duplicate, and rows would be pushed (and
/// synced) with the session's title, parent or last timestamp missing. Read whole under the
/// locks, it is also never half from before a wipe and half from after.
///
/// For the same reason a warm-up whose repair or any read fails is not taken as a partial (or
/// empty) view: it releases the locks, waits a backoff and warms up again, until it has every
/// read, as capture waits out an unavailable store rather than read past it. Nothing of the
/// session is captured (or checkpointed) meanwhile, so no line is lost; a sidecar that keeps
/// failing holds this harness's capture, as an unavailable store does.
///
/// A wipe after the warm-up does not make it stale: the replay that ends it restores every row
/// of the session from the record store, which is all the warm-up read, along with the rows
/// capture pushed since, which the enricher has counted itself.
pub(super) async fn warm(
    sink: &Sink,
    enricher: &mut MessageEnricher,
    session: &SessionId,
    start: Start,
) {
    enricher.restart(session);
    if start == Start::Beginning {
        return;
    }
    let handle = enricher.handle(session);
    let backoff = sink.warm_backoff();
    let mut state = sink.state.clone();
    let mut failures = 0;
    loop {
        let Ok(locks) = sink.lock_ready().await else {
            tracing::debug!(%session, "ai-session store unavailable; warm-up paused until ready");
            if state.wait_for(|state| *state == StoreState::Ready).await.is_err() {
                // The store's state can no longer change: nothing will be appended either.
                return;
            }
            continue;
        };
        match read_warm(sink, &handle, locks).await {
            Ok(warmed) => {
                let Warmed {
                    row,
                    titles,
                    last,
                    synthetic,
                } = warmed;
                enricher.resume(session, row.as_ref(), &titles, last.as_ref(), &synthetic);
                return;
            }
            Err(e) => {
                failures += 1;
                let delay = backoff.delay(failures);
                tracing::warn!(
                    ?e,
                    %session,
                    ?delay,
                    "failed to warm a resumed ai-session up from the sidecar; retrying"
                );
                tokio::time::sleep(delay).await;
            }
        }
    }
}

/// What a resumed session's warm-up reads from the sidecar.
struct Warmed {
    row: Option<Session>,
    titles: Vec<TitleChange>,
    last: Option<Message>,
    synthetic: Vec<SourceId>,
}

#[derive(Debug, thiserror::Error)]
enum WarmError {
    #[error(transparent)]
    Repair(#[from] AppendError),
    #[error(transparent)]
    Read(#[from] DbError),
    #[cfg(test)]
    #[error("injected warm-up failure")]
    Injected,
}

/// Read what [`warm`] needs from the sidecar, holding capture's `locks` (released on return,
/// whether the reads failed or not).
async fn read_warm(
    sink: &Sink,
    handle: &HarnessSession,
    (mut pending, local): CaptureLocks,
) -> Result<Warmed, WarmError> {
    // A row capture pushed but did not project is one of the session's the reads must see.
    #[cfg(test)]
    inject(sink, WarmStep::Repair).await?;
    sink.repair(&mut pending).await?;
    #[cfg(test)]
    inject(sink, WarmStep::Session).await?;
    let row = sink.sidecar.get_session(handle).await?;
    #[cfg(test)]
    inject(sink, WarmStep::Last).await?;
    let last = sink.sidecar.last_message(handle).await?;
    #[cfg(test)]
    inject(sink, WarmStep::Synthetic).await?;
    let synthetic = sink.sidecar.source_ids_with_prefix(handle, SYNTHETIC).await?;
    #[cfg(test)]
    inject(sink, WarmStep::Titles).await?;
    let titles = sink.sidecar.title_changes(handle).await?;
    drop(local);
    drop(pending);
    Ok(Warmed {
        row,
        titles,
        last,
        synthetic,
    })
}

/// Fail the warm-up's `step` if the test hooks say so.
#[cfg(test)]
async fn inject(sink: &Sink, step: WarmStep) -> Result<(), WarmError> {
    match sink.hook(super::hooks::Point::WarmRead(step)).await {
        super::hooks::Fault::None => Ok(()),
        _ => Err(WarmError::Injected),
    }
}

/// The checkpoint stored for a session, if any; the session checks it against its source itself.
async fn checkpoint_of(sink: &Sink, kind: HarnessKind, session: &SessionId) -> Option<Checkpoint> {
    sink.sidecar.checkpoint(&handle_of(kind, session)).await.unwrap_or_else(|e| {
        tracing::warn!(?e, %session, "failed to read ai-session checkpoint; reading from the start");
        None
    })
}

fn handle_of(kind: HarnessKind, session: &SessionId) -> HarnessSession {
    HarnessSession {
        harness: kind,
        session: NativeSessionId::from(session.to_string()),
    }
}

impl Drop for SessionCaptureEngine {
    fn drop(&mut self) {
        for listener in &self.listeners {
            listener.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    const fn at(at: u64) -> Checkpoint {
        Checkpoint {
            at,
            digest: 7,
            generation: 0,
        }
    }

    async fn sink() -> Sink {
        use atuin_client::ai_session::{AiSessionDatabase, AiSessionStore};
        use atuin_client::record::sqlite_store::SqliteStore;
        use atuin_common::encryption::paseto_v4::Key;
        use atuin_domain::record::HostId;

        let store = SqliteStore::in_memory(super::super::NOP_STORE_TIMEOUT).await.unwrap();
        let records = AiSessionStore::builder()
            .store(store)
            .host_id(HostId(atuin_common::utils::uuid_v7()))
            .key(Key::generate())
            .build();
        Sink::new(records, AiSessionDatabase::in_memory().await.unwrap())
    }

    /// After a restart the session still knows every source's title: a generated title, then
    /// a name, then the name cleared once capture resumed, shows the generated title again.
    #[rstest]
    #[tokio::test]
    async fn a_resumed_session_falls_back_to_a_title_from_before_the_restart() {
        let sink = sink().await;
        let session = SessionId::from("s1".to_owned());
        let line = |raw: serde_json::Value| AnyMessage::Ccode(serde_json::from_value(raw).unwrap());
        let title = |kind: &str, field: &str, text: &str| {
            line(serde_json::json!({"type": kind, (field): text, "sessionId": "s1"}))
        };

        let mut before = MessageEnricher::new(HarnessKind::ClaudeCode);
        for m in [
            line(serde_json::json!({
                "type": "user", "uuid": "u0", "sessionId": "s1",
                "timestamp": "2026-09-18T10:00:00Z", "message": {"role": "user", "content": "hi"},
            })),
            title("ai-title", "aiTitle", "Draft"),
            title("custom-title", "customTitle", "Mine"),
        ] {
            for row in before.capture(&session, &m) {
                sink.append(row).await.unwrap();
            }
        }

        let mut after = MessageEnricher::new(HarnessKind::ClaudeCode);
        warm(&sink, &mut after, &session, Start::Resumed).await;
        let cleared =
            after.capture(&session, &title("custom-title", "customTitle", "")).pop().unwrap();
        assert_eq!(cleared.session_title.as_deref(), Some("Draft"));
    }

    /// A live transcript of untimed lines alone (a Claude Code title line) is stored, and
    /// checkpointed past, once it has been quiet for the grace period -- not only when the
    /// daemon stops.
    #[rstest]
    #[tokio::test]
    async fn a_quiet_session_of_untimed_lines_is_stored() {
        const GRACE: Duration = Duration::from_millis(300);
        let sink = sink().await;
        let session = SessionId::from("s1".to_owned());
        let handle = handle_of(HarnessKind::ClaudeCode, &session);
        let title = AnyMessage::Ccode(
            serde_json::from_value(serde_json::json!({
                "type": "ai-title", "aiTitle": "Draft", "sessionId": "s1",
            }))
            .unwrap(),
        );
        let (tx, rx) = futures::channel::mpsc::unbounded();
        tx.unbounded_send(Ok(SessionEvent {
            session,
            checkpoint: at(40),
            message: title,
        }))
        .unwrap();

        let opened = Opened::default();
        let run = capture(HarnessKind::ClaudeCode, &sink, rx, &opened, GRACE);
        let check = async {
            tokio::time::sleep(GRACE / 10).await;
            assert!(sink.sidecar.get_session(&handle).await.unwrap().is_none(), "flushed early");
            tokio::time::sleep(GRACE * 2).await;
            let row = sink.sidecar.get_session(&handle).await.unwrap().expect("never stored");
            assert_eq!(row.title.as_deref(), Some("Draft"));
            assert_eq!(sink.sidecar.checkpoint(&handle).await.unwrap(), Some(at(40)));
            drop(tx);
        };
        tokio::join!(run, check);
    }

    /// Byte offsets and row seqs alike: only a first event strictly past the checkpoint handed
    /// to the read, in the reader's first read of the source, is a resume.
    #[rstest]
    #[case::no_checkpoint(None, at(40), Start::Beginning)]
    #[case::first_item_before_it(Some(at(100)), at(40), Start::Beginning)]
    #[case::first_item_is_it(Some(at(100)), at(100), Start::Beginning)]
    #[case::next_item(Some(at(100)), at(160), Start::Resumed)]
    #[case::first_row_seq_zero(Some(at(0)), at(0), Start::Beginning)]
    #[case::next_row_seq(Some(at(4)), at(5), Start::Resumed)]
    #[case::started_over_by_the_reader(Some(at(100)), Checkpoint { generation: 1, ..at(160) },
        Start::Beginning)]
    fn a_read_resumed_only_when_its_first_event_is_past_its_checkpoint(
        #[case] from: Option<Checkpoint>,
        #[case] first: Checkpoint,
        #[case] expected: Start,
    ) {
        assert_eq!(Start::of(from, first), expected);
    }

    fn line(uuid: &str) -> String {
        serde_json::json!({
            "type": "user", "uuid": uuid, "sessionId": "s1",
            "timestamp": "2026-09-18T10:00:00Z",
            "message": {"role": "user", "content": "hi"},
        })
        .to_string()
            + "\n"
    }

    /// Where a read of the transcript at `path` handed `from` starts, as the engine tells it.
    async fn start_of(path: &std::path::Path, from: Option<Checkpoint>) -> Start {
        use atuin_common::harnesstools::ccode::session::CcodeSession;
        use atuin_common::harnesstools::session::Session as _;

        let session = CcodeSession::open(
            SessionId::from("s1".to_owned()),
            path.to_owned(),
            BlockingPool::new(std::num::NonZeroUsize::MIN),
        );
        let events = session.messages_from(from);
        futures::pin_mut!(events);
        let (first, _) = events.next().await.unwrap().unwrap();
        Start::of(from, first)
    }

    /// Fails the next this many captures, before their push (as a panic does).
    #[derive(Debug)]
    struct FailCaptures(std::sync::atomic::AtomicUsize);

    impl super::super::hooks::Hooks for FailCaptures {
        fn at(
            &self,
            point: super::super::hooks::Point,
        ) -> futures::future::BoxFuture<'_, super::super::hooks::Fault> {
            use std::sync::atomic::Ordering;

            use super::super::hooks::{Fault, Point};

            let fail = point == Point::CapturePushing
                && self
                    .0
                    .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
                    .is_ok();
            Box::pin(async move {
                if fail {
                    Fault::Panic
                } else {
                    Fault::None
                }
            })
        }
    }

    /// A transcript replaced under its read leaves rows waiting for a timestamp, stored then;
    /// one whose append fails can't be read again (its line is gone), so it is held, and retried
    /// with the session's next rows, and the checkpoint doesn't move past it until it is stored.
    #[rstest]
    #[tokio::test]
    async fn a_row_of_a_replaced_transcript_that_fails_to_store_is_held_until_stored() {
        let mut sink = sink().await;
        sink.hooks = Some(Arc::new(FailCaptures(2.into())));
        let session = SessionId::from("s1".to_owned());
        let handle = handle_of(HarnessKind::ClaudeCode, &session);
        let line = |raw: &str| AnyMessage::Ccode(serde_json::from_str(raw).unwrap());
        let mut enricher = MessageEnricher::new(HarnessKind::ClaudeCode);
        // A title before any timestamp: it waits for one.
        assert!(enricher.capture(&session, &line(&title("mine"))).is_empty());
        assert!(enricher.has_untimed(&session));
        let (mut stuck, mut held, mut untimed) = (HashSet::new(), Held::new(), HashMap::new());

        restarted(&sink, &mut enricher, &mut stuck, &mut held, &mut untimed, &session).await;
        assert_eq!(held.get(&session).map(Vec::len), Some(1), "held, not dropped");
        assert!(stored(&sink).await.is_empty());

        // Retried first, failing again: the next row is stored, but not checkpointed past.
        let rows = enricher.capture(&session, &line(&turn("u1", None, 1)));
        store(&sink, &enricher, &mut stuck, &mut held, &session, rows, at(100)).await;
        assert_eq!(held.get(&session).map(Vec::len), Some(1));
        assert_eq!(stored(&sink).await, ["u1"]);
        assert_eq!(sink.sidecar.checkpoint(&handle).await.unwrap(), None);

        // Stored at last: the checkpoint moves on.
        let rows = enricher.capture(&session, &line(&turn("u2", Some("u1"), 2)));
        store(&sink, &enricher, &mut stuck, &mut held, &session, rows, at(200)).await;
        assert!(held.is_empty());
        let ids = stored(&sink).await;
        assert_eq!(ids.len(), 3, "{ids:?}");
        assert!(ids.iter().any(|id| id.starts_with(SYNTHETIC)), "the title: {ids:?}");
        assert_eq!(sink.sidecar.checkpoint(&handle).await.unwrap(), Some(at(200)));
    }

    /// The source ids stored for session `s1`, sorted.
    async fn stored(sink: &Sink) -> Vec<String> {
        use futures::TryStreamExt;

        let handle = handle_of(HarnessKind::ClaudeCode, &SessionId::from("s1".to_owned()));
        let rows: Vec<Message> = sink.sidecar.messages(&handle).try_collect().await.unwrap();
        let mut ids: Vec<String> = rows.into_iter().map(|m| m.source_id.to_string()).collect();
        ids.sort();
        ids
    }

    /// A transcript line of session `s1`: `uuid` hanging from `parent`, at second `at`.
    fn turn(uuid: &str, parent: Option<&str>, at: u32) -> String {
        said(uuid, parent, at, &format!("said {uuid}"))
    }

    /// [`turn`], saying `text`.
    fn said(uuid: &str, parent: Option<&str>, at: u32, text: &str) -> String {
        serde_json::json!({
            "type": "user", "uuid": uuid, "parentUuid": parent, "sessionId": "s1",
            "timestamp": format!("2026-09-18T10:00:{at:02}Z"),
            "message": {"role": "user", "content": text},
        })
        .to_string()
            + "\n"
    }

    fn title(text: &str) -> String {
        serde_json::json!({"type": "custom-title", "customTitle": text, "sessionId": "s1"})
            .to_string()
            + "\n"
    }

    /// A captured transcript replaced in place (written out again along another branch, as
    /// `atuin ai resume` switching it does: a new file renamed over it, its title line kept) is
    /// read again from its start while it is followed: the rows already stored are not stored
    /// again (not even its title line, which capture keys on its content and counts), none is
    /// lost, the new branch's rows are stored, and the checkpoint is the new file's end, not a
    /// stale offset past it. Shrunk in place, it is read again from its start too. The line
    /// reader says it started over, so it is told even when the new file's first line ends past
    /// where the old one was read to, which no comparison of positions could tell from lines
    /// appended.
    #[rstest]
    #[case::renamed_over(false, false)]
    #[case::truncated_in_place(true, false)]
    #[case::renamed_over_with_a_first_line_past_what_was_read(false, true)]
    #[tokio::test]
    async fn a_followed_transcript_replaced_in_place_is_captured_once(
        #[case] in_place: bool,
        #[case] long_first_line: bool,
    ) {
        use atuin_common::harnesstools::ccode::session::CcodeSessions;
        use atuin_common::harnesstools::session::{Listener as _, Sessions as _};

        const WAIT: Duration = Duration::from_secs(20);
        let root = tempfile::tempdir().unwrap();
        let project = root.path().join("-work-proj");
        std::fs::create_dir(&project).unwrap();
        let path = project.join("s1.jsonl");
        let old = turn("u1", None, 1)
            + &title("mine")
            + &turn("u2", Some("u1"), 2)
            + &turn("u4", Some("u2"), 3);
        std::fs::write(&path, &old).unwrap();

        let sink = Arc::new(sink().await);
        let listener = CcodeSessions::builder()
            .root(root.path().to_path_buf())
            .pool(BlockingPool::new(std::num::NonZeroUsize::MIN))
            .build()
            .listener()
            .unwrap();
        let opened: Opened = Arc::default();
        let checkpoint = {
            let (sink, opened) = (sink.clone(), opened.clone());
            move |id: &SessionId| {
                let (sink, opened, id) = (sink.clone(), opened.clone(), id.clone());
                async move {
                    let from = checkpoint_of(&sink, HarnessKind::ClaudeCode, &id).await;
                    opened.lock().insert(id, from);
                    from
                }
            }
        };
        let events = listener.events(checkpoint).map(|ev| {
            ev.map(|ev| SessionEvent {
                session: ev.session,
                checkpoint: ev.checkpoint,
                message: AnyMessage::Ccode(ev.message),
            })
        });
        let run = capture(HarnessKind::ClaudeCode, &sink, events, &opened, UNTIMED_GRACE);
        let check = async {
            let settle = |want: usize| {
                let sink = &sink;
                async move {
                    tokio::time::timeout(WAIT, async {
                        while stored(sink).await.len() < want {
                            tokio::time::sleep(Duration::from_millis(20)).await;
                        }
                    })
                    .await
                    .expect("capture never stored the rows");
                }
            };
            settle(4).await;
            let before = stored(&sink).await;

            // Switched to the branch off u1 that another machine went on with: shorter than
            // what was read, and holding the title line again.
            let first = if long_first_line {
                // The same row (u1), its line now longer than all that was read of the old file.
                said("u1", None, 1, &"said u1 at length ".repeat(old.len()))
            } else {
                turn("u1", None, 1)
            };
            assert_eq!(first.len() > old.len(), long_first_line);
            let new = first + &title("mine") + &turn("u3", Some("u1"), 4);
            if in_place {
                std::fs::write(&path, &new).unwrap();
            } else {
                atuin_common::fs::replace(&path, new.as_bytes(), |_| true).unwrap();
            }
            settle(5).await;
            // Whatever else it would store, it has had the time to.
            tokio::time::sleep(Duration::from_millis(500)).await;

            let after = stored(&sink).await;
            let mut want = before.clone();
            want.push("u3".to_owned());
            want.sort();
            assert_eq!(after, want, "u2 and u4 stay; the title line is not stored twice");
            assert_eq!(after.iter().filter(|id| id.starts_with(SYNTHETIC)).count(), 1);
            let handle = handle_of(HarnessKind::ClaudeCode, &SessionId::from("s1".to_owned()));
            let at = sink.sidecar.checkpoint(&handle).await.unwrap().unwrap().at;
            assert_eq!(at, new.len() as u64, "checkpointed at the new file's end");
        };
        tokio::select! {
            () = run => panic!("capture ended"),
            () = check => {}
        }
    }

    /// Against a real transcript: a checkpoint that still names its line resumes; one the file
    /// was rewritten under, whatever the length of its first line, reads from the beginning.
    #[rstest]
    #[tokio::test]
    async fn a_transcript_read_is_told_resumed_only_past_a_checkpoint_that_holds() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s1.jsonl");
        std::fs::write(&path, line("u1") + &line("u2")).unwrap();
        let first = line("u1");
        let from =
            Checkpoint::new(u64::try_from(first.len()).unwrap(), first.trim_end().as_bytes());

        assert_eq!(start_of(&path, None).await, Start::Beginning);
        assert_eq!(start_of(&path, Some(from)).await, Start::Resumed);

        std::fs::write(&path, line("v1") + &line("v2")).unwrap();
        assert_eq!(start_of(&path, Some(from)).await, Start::Beginning, "rewritten");

        std::fs::write(&path, line("x") + &line("u2")).unwrap();
        assert_eq!(start_of(&path, Some(from)).await, Start::Beginning, "shorter first line");

        // Its first line ends past the checkpoint, which no position could tell from a resume:
        // the reader says it started over.
        std::fs::write(&path, line(&"w".repeat(first.len())) + &line("u2")).unwrap();
        assert_eq!(start_of(&path, Some(from)).await, Start::Beginning, "longer first line");
    }
}
