mod checkpoint;
pub mod error;
pub mod model;
pub mod patch;
pub mod synthetic;

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

pub use checkpoint::Checkpoint;
use enum_dispatch::enum_dispatch;
pub use error::{CaptureError, MessageError, RuntimeError, WatchError};
use futures::stream::AbortHandle;
use futures::{Stream, StreamExt, TryStreamExt};
pub use model::{
    Content, MessageId, ParentKind, Role, SessionEvent, SessionId, StopReason, TitleChange,
    TitleSource, ToolCallId, ToolResult, ToolUse, UNREADABLE_SUMMARY, Usage, is_substantive,
    is_tool_output,
};
pub use patch::{Change, FilePatch, Hunk, Patch};
use time::OffsetDateTime;
use tokio::sync::watch;

use crate::fs::tree_watcher::FileStat;
use crate::sync::BlockingPool;

/// Recursively scan `root` for session files, calling `accept(path, is_file)` on each non-directory
/// entry to build a session. Directory, entry, and file-type read failures are surfaced as `Err`
/// (never silently dropped) so a one-shot [`Sessions::existing`] scan can report a partial result.
/// Symlinks are not followed (a symlinked directory is neither pushed nor accepted), matching the
/// live watcher.
pub(crate) fn scan_sessions<S>(
    root: PathBuf,
    accept: impl Fn(&Path, bool) -> Option<S>,
) -> Vec<Result<S, RuntimeError>> {
    let mut out = Vec::new();
    let mut stack = vec![root];
    while let Some(dir) = stack.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(e) => {
                out.push(Err(RuntimeError::Io(e)));
                continue;
            }
        };
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(e) => {
                    out.push(Err(RuntimeError::Io(e)));
                    continue;
                }
            };
            let path = entry.path();
            let file_type = match entry.file_type() {
                Ok(file_type) => file_type,
                Err(e) => {
                    out.push(Err(RuntimeError::Io(e)));
                    continue;
                }
            };
            if file_type.is_dir() {
                stack.push(path);
            } else if let Some(session) = accept(&path, file_type.is_file()) {
                out.push(Ok(session));
            }
        }
    }
    out
}

#[enum_dispatch]
pub trait Message: Send + 'static {
    fn id(&self) -> Option<MessageId>;
    fn role(&self) -> Role;
    fn timestamp(&self) -> Option<OffsetDateTime>;
    fn content(&self) -> Vec<Content>;
    fn model(&self) -> Option<String> {
        None
    }
    /// Who served [`Self::model`].
    fn provider(&self) -> Option<String> {
        None
    }
    fn usage(&self) -> Option<Usage> {
        None
    }
    fn stop_reason(&self) -> Option<StopReason> {
        None
    }
    fn cwd(&self) -> Option<std::path::PathBuf> {
        None
    }
    fn git_branch(&self) -> Option<String> {
        None
    }
    fn parent_id(&self) -> Option<MessageId> {
        None
    }
    /// The id of the row before this line, for a harness whose lines name none ([`Self::parent_id`]
    /// is `None` throughout): capture links each of its rows to the row it captured before, and
    /// this says what that is where capture has seen nothing yet, but the reader knows (a read
    /// resumed past a checkpoint, a transcript continuing another). Never part of a row's
    /// content-addressed id, so it can change without minting new rows.
    fn follows(&self) -> Option<MessageId> {
        None
    }
    /// The session this line says it belongs to, when the harness writes one per line. Differs
    /// from the file's session for a Claude Code subagent transcript, whose lines name the parent.
    fn parent_session(&self) -> Option<SessionId> {
        None
    }
    /// How this line's session relates to [`Self::parent_session`], when the line names one.
    /// `None` when the harness does not say (then readers should assume the cautious case, a
    /// subagent).
    fn parent_kind(&self) -> Option<ParentKind> {
        None
    }
    /// One model call: groups the rows a single API response is split into. Unique within the
    /// harness and unchanged when the harness copies the line into another session (forks,
    /// subagent replays), since capture counts a call's usage once across every session holding
    /// it, at the field-wise max of its rows. `Some` on every line that carries usage.
    fn turn_id(&self) -> Option<String> {
        None
    }
    /// A title this line assigns to the session, or clears.
    fn title(&self) -> Option<TitleChange> {
        None
    }
}

pub trait Session: Send + 'static {
    type Message: Message;

    fn id(&self) -> SessionId;

    /// Follow the session from just past `from`, yielding with each message the checkpoint just
    /// past it.
    ///
    /// The session reads from its beginning instead when `from` is `None`, or no longer names the
    /// item it was taken after. **Be warned**: a checkpoint's position means whatever the harness
    /// reading the session makes it mean -- a byte offset into a transcript, a row's sequence in
    /// a log -- and belongs to that harness alone: a consumer stores one and hands it back.
    fn messages_from(
        self,
        from: Option<Checkpoint>,
    ) -> impl Stream<Item = Result<(Checkpoint, Self::Message), MessageError>> + Send + 'static;

    fn messages(self) -> impl Stream<Item = Result<Self::Message, MessageError>> + Send + 'static
    where
        Self: Sized,
    {
        self.messages_from(None).map_ok(|(_, message)| message)
    }

    fn read(&self) -> impl Stream<Item = Result<Self::Message, MessageError>> + Send + 'static;

    /// The session once it has changed after `since` (at once if it has), `None` once it no
    /// longer can. At once for a session that can't tell.
    fn changed_after(self, since: SystemTime) -> impl Future<Output = Option<Self>> + Send
    where
        Self: Sized,
    {
        let _ = since;
        std::future::ready(Some(self))
    }
}

/// [`Session::changed_after`] for a session over a file `changes` follows: once the file is
/// modified after `since`, or at once without a watch or a modification time. `None` once the
/// watch has ended.
pub(crate) async fn modified_after(
    changes: Option<&mut watch::Receiver<FileStat>>,
    since: SystemTime,
) -> Option<()> {
    // File times come from a coarser clock than `since`, and are whole seconds on some
    // filesystems: a file written just after it can be stamped just before.
    const SLACK: Duration = Duration::from_secs(2);
    let Some(changes) = changes else {
        return Some(());
    };
    let since = since.checked_sub(SLACK).unwrap_or(since);
    let modified = changes.wait_for(|stat| stat.modified().is_none_or(|at| at >= since)).await;
    modified.ok().map(drop)
}

pub trait Listener {
    type Session: Session;

    fn watch(self) -> impl Stream<Item = Result<Self::Session, WatchError>> + Send + 'static;

    /// Follow every session this listener reports, each from the checkpoint the consumer stored
    /// for it.
    ///
    /// `checkpoint` gives that checkpoint, `None` for none. A session checks it itself and reads
    /// from its beginning when it no longer names what it was taken after
    /// ([`Session::messages_from`]). A session with none is read, whole, only once it changes
    /// after `since` ([`Session::changed_after`]): one nobody touches since is history, left to
    /// an import. `checkpointed` names the sessions with one stored as this starts: only those are
    /// asked for theirs at once, the rest once they change.
    fn events<F>(
        self,
        since: SystemTime,
        mut checkpointed: HashSet<SessionId>,
        checkpoint: impl Fn(&SessionId) -> F + Send + 'static,
    ) -> impl Stream<Item = Result<SessionEvent<<Self::Session as Session>::Message>, CaptureError>>
    + Send
    + 'static
    where
        Self: Sized,
        F: Future<Output = Option<Checkpoint>> + Send + 'static,
    {
        let sessions = self.watch();
        async_stream::stream! {
            let mut active = futures::stream::SelectAll::new();
            // Sessions without a checkpoint, until they change after `since`: bare waits, as
            // most of them never do.
            let mut waiting = futures::stream::FuturesUnordered::new();
            // One live read per transcript. A session the watcher reports again (the file
            // removed and recreated, or renamed back) ends its old read at once: its buffered
            // lines would otherwise interleave with the new one and could move the checkpoint
            // backwards. ponytail: a handle outlives its read, which only ends once its file is
            // removed; prune on end, guarding against a replaced read's end, if the map ever
            // matters.
            let mut handles: HashMap<SessionId, AbortHandle> = HashMap::new();
            futures::pin_mut!(sessions);
            let mut sessions_done = false;
            loop {
                if sessions_done && active.is_empty() && waiting.is_empty() {
                    break;
                }
                let start = tokio::select! {
                    biased;
                    appeared = sessions.next(), if !sessions_done => match appeared {
                        Some(Ok(session)) => {
                            let id = session.id();
                            let stored = checkpointed.contains(&id);
                            let from = if stored { checkpoint(&id).await } else { None };
                            match from {
                                Some(_) => Some((session, from)),
                                None => {
                                    if let Some(old) = handles.remove(&id) {
                                        old.abort();
                                    }
                                    waiting.push(session.changed_after(since));
                                    None
                                }
                            }
                        }
                        Some(Err(err)) => {
                            yield Err(CaptureError::from(err));
                            None
                        }
                        None => {
                            sessions_done = true;
                            None
                        }
                    },
                    changed = waiting.next(), if !waiting.is_empty() => match changed.flatten() {
                        Some(session) => {
                            // An import may have stored it meanwhile.
                            let from = checkpoint(&session.id()).await;
                            Some((session, from))
                        }
                        None => None,
                    },
                    tagged = active.next(), if !active.is_empty() => {
                        let Some((session, result)) = tagged else { continue };
                        match result {
                            Ok((at, message)) => {
                                yield Ok(SessionEvent { session, checkpoint: at, message });
                            }
                            Err(source) => yield Err(CaptureError::Message { session, source }),
                        }
                        None
                    }
                };
                let Some((session, from)) = start else { continue };
                let id = session.id();
                // Its read may store a checkpoint: reported again, it resumes from it at once.
                checkpointed.insert(id.clone());
                let (messages, handle) = futures::stream::abortable(session.messages_from(from));
                if let Some(old) = handles.insert(id.clone(), handle) {
                    old.abort();
                }
                active.push(messages.map(move |item| (id.clone(), item)).boxed());
            }
        }
    }
}

pub trait Sessions {
    type Listener: Listener;

    fn listener(&self) -> Result<Self::Listener, RuntimeError>;

    /// One-shot scan of every session under the root. Yields `Err` for a directory, entry, or
    /// file-type read that fails mid-scan, so a caller (e.g. import) can report a partial scan
    /// rather than silently treating it as complete. The outer `Err` is only the root itself
    /// being unreadable.
    fn existing(
        &self,
    ) -> Result<
        impl Stream<Item = Result<<Self::Listener as Listener>::Session, RuntimeError>> + Send + 'static,
        RuntimeError,
    >;
}

pub trait Observable {
    type Sessions: Sessions;

    /// The harness's sessions, whose file reads all run in `pool`.
    fn sessions(&self, pool: BlockingPool) -> Self::Sessions;
}

pub mod prelude {
    pub use super::{Listener, Message, Observable, Session, Sessions};
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroUsize;

    use rstest::rstest;
    use time::OffsetDateTime;

    use super::*;
    use crate::harnesstools::ccode::session::CcodeSession;
    use crate::harnesstools::codex::session::CodexSession;
    use crate::harnesstools::pi::session::PiSession;
    use crate::harnesstools::session::model::{Content, MessageId, Role};

    #[derive(Debug, Clone)]
    struct StubMsg;

    impl Message for StubMsg {
        fn id(&self) -> Option<MessageId> {
            Some(MessageId::from("m1".to_owned()))
        }
        fn role(&self) -> Role {
            Role::Assistant
        }
        fn timestamp(&self) -> Option<OffsetDateTime> {
            None
        }
        fn content(&self) -> Vec<Content> {
            vec![Content::Text("hello".into())]
        }
    }

    #[rstest]
    fn message_trait_exposes_a_normalized_view() {
        let m = StubMsg;
        assert_eq!(m.role(), Role::Assistant);
        assert_eq!(m.content(), vec![Content::Text("hello".into())]);
        assert_eq!(m.id(), Some(MessageId::from("m1".to_owned())));
    }

    /// A transcript that yields a message at each of the given positions, then either ends or
    /// hangs like a live file waiting for more. Changed after `since` if it `changes`, else gone
    /// before it ever was.
    struct StubSession {
        id: &'static str,
        offsets: Vec<u64>,
        hang: bool,
        changes: bool,
    }

    impl Session for StubSession {
        type Message = StubMsg;

        fn id(&self) -> SessionId {
            SessionId::from(self.id.to_owned())
        }

        fn messages_from(
            self,
            _from: Option<Checkpoint>,
        ) -> impl Stream<Item = Result<(Checkpoint, StubMsg), MessageError>> + Send + 'static
        {
            let items = futures::stream::iter(self.offsets.into_iter().map(|at| {
                Ok((
                    Checkpoint {
                        at,
                        digest: 0,
                        generation: 0,
                    },
                    StubMsg,
                ))
            }));
            if self.hang {
                items.chain(futures::stream::pending()).left_stream()
            } else {
                items.right_stream()
            }
        }

        fn read(&self) -> impl Stream<Item = Result<StubMsg, MessageError>> + Send + 'static {
            futures::stream::empty()
        }

        fn changed_after(self, _since: SystemTime) -> impl Future<Output = Option<Self>> + Send {
            std::future::ready(self.changes.then_some(self))
        }
    }

    /// Reports each session 50 ms after the one before, as a watcher does for a transcript that
    /// is removed and recreated. The tests pause time, so that is once everything before it has
    /// settled.
    struct StubListener {
        reports: Vec<StubSession>,
    }

    impl Listener for StubListener {
        type Session = StubSession;

        fn watch(self) -> impl Stream<Item = Result<StubSession, WatchError>> + Send + 'static {
            futures::stream::iter(self.reports).then(|session| async {
                tokio::time::sleep(Duration::from_millis(50)).await;
                Ok(session)
            })
        }
    }

    const ANY: Checkpoint = Checkpoint {
        at: 0,
        digest: 0,
        generation: 0,
    };

    /// A session reported twice keeps only its newest read: the first hangs like a live
    /// transcript, and without replacement `events()` would never end.
    #[rstest]
    #[tokio::test(start_paused = true)]
    async fn a_reported_again_session_replaces_its_earlier_read(
        #[values(false, true)] stored: bool,
    ) {
        let report = |at, hang| StubSession {
            id: "s",
            offsets: vec![at],
            hang,
            changes: true,
        };
        let listener = StubListener {
            reports: vec![report(1, true), report(2, false)],
        };
        let checkpointed = stored.then(|| SessionId::from("s".to_owned())).into_iter().collect();
        let from = stored.then_some(ANY);
        let events = listener
            .events(SystemTime::UNIX_EPOCH, checkpointed, move |_| std::future::ready(from));
        let offsets: Vec<u64> = tokio::time::timeout(
            Duration::from_secs(5),
            events.map(|ev| ev.unwrap().checkpoint.at).collect(),
        )
        .await
        .expect("a replaced read must not keep events() alive");
        assert_eq!(offsets, vec![1, 2]);
    }

    /// Only a session with a checkpoint stored is asked for it at once; any other is asked, and
    /// read, once it changes after `since`, and never if it doesn't.
    #[rstest]
    #[tokio::test(start_paused = true)]
    async fn a_session_without_a_checkpoint_is_asked_for_one_only_once_it_changes() {
        let session = |id, at, changes| StubSession {
            id,
            offsets: vec![at],
            hang: false,
            changes,
        };
        let listener = StubListener {
            reports: vec![
                session("stored", 1, false),
                session("untouched", 2, false),
                session("changed", 3, true),
            ],
        };
        let asked = std::sync::Arc::new(parking_lot::Mutex::new(Vec::new()));
        let checkpoint = {
            let asked = std::sync::Arc::clone(&asked);
            move |id: &SessionId| {
                asked.lock().push(id.to_string());
                std::future::ready(Some(ANY))
            }
        };
        let checkpointed = HashSet::from([SessionId::from("stored".to_owned())]);
        let events = listener.events(SystemTime::UNIX_EPOCH, checkpointed, checkpoint);
        let read: Vec<u64> = tokio::time::timeout(
            Duration::from_secs(5),
            events.map(|ev| ev.unwrap().checkpoint.at).collect(),
        )
        .await
        .expect("events() did not end");
        assert_eq!(read, vec![1, 3]);
        assert_eq!(*asked.lock(), ["stored", "changed"]);
    }

    #[derive(Debug, Clone, Copy)]
    enum JsonlHarness {
        Ccode,
        Codex,
        Pi,
    }

    impl JsonlHarness {
        /// The checkpoints of the messages this harness's session over `path` reads from `from`.
        async fn resumed(self, path: PathBuf, from: Option<Checkpoint>) -> Vec<Checkpoint> {
            let id = SessionId::from("s".to_owned());
            let pool = BlockingPool::new(NonZeroUsize::MIN);
            let checkpoints = match self {
                Self::Ccode => CcodeSession::open(id, path, pool)
                    .messages_from(from)
                    .map_ok(|(at, _)| at)
                    .boxed(),
                Self::Codex => CodexSession::open(id, path, pool)
                    .messages_from(from)
                    .map_ok(|(at, _)| at)
                    .boxed(),
                Self::Pi => {
                    PiSession::open(id, path, pool).messages_from(from).map_ok(|(at, _)| at).boxed()
                }
            };
            checkpoints.try_collect().await.unwrap()
        }
    }

    const A: &str = r#"{"type":"a"}"#;
    const B: &str = r#"{"type":"b"}"#;
    const X: &str = r#"{"type":"x"}"#;
    const Y: &str = r#"{"type":"y"}"#;

    /// Each line is 12 bytes, so lines end at 13 and 26.
    #[rstest]
    #[case::untouched(
        &[A, B], Some(Checkpoint::new(13, A.as_bytes())), vec![Checkpoint::new(26, B.as_bytes())])]
    #[case::without_a_checkpoint(
        &[A, B], None, vec![Checkpoint::new(13, A.as_bytes()), Checkpoint::new(26, B.as_bytes())])]
    #[case::rewritten_to_the_same_length(
        &[X, Y],
        Some(Checkpoint::new(13, A.as_bytes())),
        vec![Checkpoint::new(13, X.as_bytes()), Checkpoint::new(26, Y.as_bytes())],
    )]
    #[case::cut_shorter(
        &[A], Some(Checkpoint::new(26, B.as_bytes())), vec![Checkpoint::new(13, A.as_bytes())])]
    #[tokio::test]
    async fn a_transcript_resumes_only_where_its_checkpoint_still_holds(
        #[values(JsonlHarness::Ccode, JsonlHarness::Codex, JsonlHarness::Pi)] harness: JsonlHarness,
        #[case] lines: &[&str],
        #[case] from: Option<Checkpoint>,
        #[case] expected: Vec<Checkpoint>,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.jsonl");
        std::fs::write(&path, lines.join("\n") + "\n").unwrap();
        assert_eq!(harness.resumed(path, from).await, expected);
    }
}

pub mod any;

use crate::harnesstools::ccode::session::CcodeMessage;
use crate::harnesstools::codex::session::CodexMessage;
use crate::harnesstools::opencode::session::OpencodeMessage;
use crate::harnesstools::pi::session::PiMessage;

#[enum_dispatch(Message)]
#[derive(Debug, Clone)]
pub enum AnyMessage {
    Ccode(CcodeMessage),
    Codex(CodexMessage),
    Opencode(OpencodeMessage),
    Pi(PiMessage),
}
