mod checkpoint;
pub mod error;
pub mod model;

use std::future::Future;
use std::path::{Path, PathBuf};

pub use checkpoint::Checkpoint;
use enum_dispatch::enum_dispatch;
pub use error::{CaptureError, MessageError, RuntimeError, WatchError};
use futures::{Stream, StreamExt, TryStreamExt};
pub use model::{
    Content, MessageId, Role, SessionEvent, SessionId, StopReason, TitleChange, TitleSource,
    ToolCallId, ToolResult, ToolUse, Usage,
};
use time::OffsetDateTime;

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
    /// The session this line says it belongs to, when the harness writes one per line. Differs
    /// from the file's session for a Claude Code subagent transcript, whose lines name the parent.
    fn parent_session(&self) -> Option<SessionId> {
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
}

pub trait Listener {
    type Session: Session;

    fn watch(self) -> impl Stream<Item = Result<Self::Session, WatchError>> + Send + 'static;

    /// Follow every session this listener reports, each from the checkpoint the consumer stored
    /// for it.
    ///
    /// `checkpoint` gives that checkpoint, `None` for none. A session checks it itself and reads
    /// from its beginning when it no longer names what it was taken after
    /// ([`Session::messages_from`]).
    fn events<F>(
        self,
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
            // One live stream per transcript. A session the watcher reports again (the file
            // removed and recreated, or renamed back) replaces the old stream: its buffered
            // lines would otherwise interleave with the new one and could move the checkpoint
            // backwards. ponytail: handles are kept for every session ever seen; prune on end
            // if the map ever matters.
            let mut handles: std::collections::HashMap<SessionId, futures::stream::AbortHandle> =
                std::collections::HashMap::new();
            futures::pin_mut!(sessions);
            let mut sessions_done = false;
            loop {
                if sessions_done && active.is_empty() {
                    break;
                }
                tokio::select! {
                    biased;
                    appeared = sessions.next(), if !sessions_done => match appeared {
                        Some(Ok(session)) => {
                            let id = session.id();
                            let from = checkpoint(&id).await;
                            let tag = id.clone();
                            let (tagged, handle) = futures::stream::abortable(
                                session.messages_from(from).map(move |item| (tag.clone(), item)),
                            );
                            if let Some(old) = handles.insert(id, handle) {
                                old.abort();
                            }
                            active.push(tagged.boxed());
                        }
                        Some(Err(err)) => yield Err(CaptureError::from(err)),
                        None => sessions_done = true,
                    },
                    tagged = active.next(), if !active.is_empty() => {
                        if let Some((session, result)) = tagged {
                            match result {
                                Ok((at, message)) => {
                                    yield Ok(SessionEvent { session, checkpoint: at, message });
                                }
                                Err(source) => yield Err(CaptureError::Message { session, source }),
                            }
                        }
                    }
                }
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
    /// hangs like a live file waiting for more.
    struct StubSession {
        id: &'static str,
        offsets: Vec<u64>,
        hang: bool,
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
            let items = futures::stream::iter(
                self.offsets.into_iter().map(|at| Ok((Checkpoint { at, digest: 0 }, StubMsg))),
            );
            if self.hang {
                items.chain(futures::stream::pending()).left_stream()
            } else {
                items.right_stream()
            }
        }

        fn read(&self) -> impl Stream<Item = Result<StubMsg, MessageError>> + Send + 'static {
            futures::stream::empty()
        }
    }

    /// Reports `first` at once and `second` shortly after, as a watcher does for a transcript
    /// that is removed and recreated.
    struct StubListener {
        first: StubSession,
        second: StubSession,
    }

    impl Listener for StubListener {
        type Session = StubSession;

        fn watch(self) -> impl Stream<Item = Result<StubSession, WatchError>> + Send + 'static {
            let second = self.second;
            futures::stream::iter([Ok(self.first)]).chain(futures::stream::once(async move {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                Ok(second)
            }))
        }
    }

    /// A session reported twice keeps only its newest stream: the first stream hangs like a
    /// live transcript, and without replacement `events()` would never end.
    #[rstest]
    #[tokio::test]
    async fn a_reported_again_session_replaces_its_earlier_stream() {
        let listener = StubListener {
            first: StubSession {
                id: "s",
                offsets: vec![1],
                hang: true,
            },
            second: StubSession {
                id: "s",
                offsets: vec![2],
                hang: false,
            },
        };
        let events = listener.events(|_| async { None });
        let offsets: Vec<u64> = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            events.map(|ev| ev.unwrap().checkpoint.at).collect(),
        )
        .await
        .expect("the replaced stream must not keep events() alive");
        assert_eq!(offsets, vec![1, 2]);
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
