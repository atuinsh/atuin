use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use atuin_client::ai_session::{HarnessKind, HarnessSession, Message, NativeSessionId};
use atuin_common::harnesstools::AnyHarness;
use atuin_common::harnesstools::session::{
    AnyMessage, CaptureError, Checkpoint, RuntimeError, SessionEvent, SessionId,
};
use atuin_common::sync::BlockingPool;
use futures::{Stream, StreamExt};
use parking_lot::Mutex;
use tokio::task::JoinHandle;
use tokio::time::Instant;

use super::Sink;
use super::message_enricher::{MessageEnricher, SYNTHETIC};

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
    pub fn nop() -> Self {
        Self {
            listeners: Vec::new(),
        }
    }

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
    let mut enricher = MessageEnricher::new(kind);
    // Sessions with a failed append: their checkpoint must not move past the line that was
    // lost, or a restart would never re-read it.
    let mut stuck: HashSet<SessionId> = HashSet::new();
    // Sessions holding rows that wait for a timestamp: when they may be flushed, and the
    // checkpoint past the last line seen.
    let mut untimed: HashMap<SessionId, (Instant, Checkpoint)> = HashMap::new();

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
                if let Some(from) = opened {
                    let start = Start::of(from, checkpoint);
                    // A new read retries whatever a failed append lost.
                    stuck.remove(&session);
                    untimed.remove(&session);
                    warm(sink, &mut enricher, &session, start).await;
                }
                let rows = enricher.capture(&session, &message);
                if rows.is_empty() {
                    if enricher.has_untimed(&session) {
                        untimed.insert(session, (Instant::now() + grace, checkpoint));
                    }
                    continue;
                }
                untimed.remove(&session);
                store(sink, &enricher, &mut stuck, &session, rows, checkpoint).await;
            }
            () = tokio::time::sleep_until(due.unwrap_or_else(Instant::now)), if due.is_some() => {
                let now = Instant::now();
                let quiet: Vec<SessionId> =
                    untimed.iter().filter(|(_, (at, _))| *at <= now).map(|(s, _)| s.clone()).collect();
                for session in quiet {
                    let Some((_, checkpoint)) = untimed.remove(&session) else { continue };
                    let rows = enricher.finish(&session);
                    store(sink, &enricher, &mut stuck, &session, rows, checkpoint).await;
                }
            }
        }
    }
}

/// Append a session's rows, then checkpoint just past the line that completed them unless an
/// append failed.
async fn store(
    sink: &Sink,
    enricher: &MessageEnricher,
    stuck: &mut HashSet<SessionId>,
    session: &SessionId,
    rows: Vec<Message>,
    checkpoint: Checkpoint,
) {
    for msg in rows {
        if let Err(e) = sink.append(msg).await {
            tracing::warn!(?e, "failed to capture ai-session message");
            stuck.insert(session.clone());
        }
    }
    if stuck.contains(session) {
        return;
    }
    // ponytail: one checkpoint write per row; batch per session on idle if it shows up in
    // profiles.
    if let Err(e) = sink.sidecar.set_checkpoint(&enricher.handle(session), checkpoint).await {
        tracing::warn!(?e, "failed to record ai-session checkpoint");
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
    /// The converse is not exact: a source rewritten so that its first item now ends past `from`
    /// (a first line longer than the old checkpoint's offset; an opencode aggregate recreated
    /// whose first row with a message has a greater `seq`) is read from its beginning but taken
    /// for a resume. That errs on the safe side: the bookkeeping is then warmed from rows of the
    /// old content, so a re-read id-less line that matches one of them is stored again under the
    /// next ordinal rather than deduplicated. The other mistake, a resume taken for the
    /// beginning, would reset the ordinals and silently drop genuinely new repeats of earlier
    /// lines, and cannot happen.
    pub(super) fn of(from: Option<Checkpoint>, first: Checkpoint) -> Self {
        match from {
            Some(from) if first.at > from.at => Self::Resumed,
            _ => Self::Beginning,
        }
    }
}

/// The checkpoint handed to each session `events()` has (re)opened and not yielded from since.
type Opened = Arc<Mutex<HashMap<SessionId, Option<Checkpoint>>>>;

/// Set a session's bookkeeping up for a fresh read of its transcript: empty for one read from
/// the beginning, whose every line replays; warmed from the sidecar for one resumed past its
/// start, which replays none of the lines before it.
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
    let row = sink.sidecar.get_session(&handle).await.unwrap_or_else(|e| {
        tracing::warn!(?e, %session, "failed to load the ai-session row");
        None
    });
    let last = sink.sidecar.last_message(&handle).await.unwrap_or_else(|e| {
        tracing::warn!(?e, %session, "failed to load the last ai-session message");
        None
    });
    let synthetic =
        sink.sidecar.source_ids_with_prefix(&handle, SYNTHETIC).await.unwrap_or_else(|e| {
            tracing::warn!(?e, %session, "failed to load the ai-session synthetic ids");
            Vec::new()
        });
    let titles = sink.sidecar.title_changes(&handle).await.unwrap_or_else(|e| {
        tracing::warn!(?e, %session, "failed to load the ai-session title changes");
        Vec::new()
    });
    enricher.resume(session, row.as_ref(), &titles, last.as_ref(), &synthetic);
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
        Checkpoint { at, digest: 7 }
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
    /// to the read is a resume.
    #[rstest]
    #[case::no_checkpoint(None, at(40), Start::Beginning)]
    #[case::first_item_before_it(Some(at(100)), at(40), Start::Beginning)]
    #[case::first_item_is_it(Some(at(100)), at(100), Start::Beginning)]
    #[case::next_item(Some(at(100)), at(160), Start::Resumed)]
    #[case::first_row_seq_zero(Some(at(0)), at(0), Start::Beginning)]
    #[case::next_row_seq(Some(at(4)), at(5), Start::Resumed)]
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

    /// Against a real transcript: a checkpoint that still names its line resumes; one the file
    /// was rewritten under, with a first line of the same length or shorter, reads from the
    /// beginning.
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
    }
}
