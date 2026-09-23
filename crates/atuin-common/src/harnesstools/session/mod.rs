pub mod error;
pub mod model;

use std::future::Future;
use std::path::{Path, PathBuf};

use enum_dispatch::enum_dispatch;
pub use error::{CaptureError, MessageError, RuntimeError, WatchError};
use futures::{Stream, StreamExt, TryStreamExt};
pub use model::{
    Content, MessageId, Role, SessionEvent, SessionId, StopReason, ToolCallId, ToolResult, ToolUse,
    Usage,
};
use time::OffsetDateTime;

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
    /// What the harness calls this message within its session: a stable identity, not a key
    /// unique to one message.
    ///
    /// A message supersedes an earlier one of the same id only where it carries the higher
    /// [`revision`](Message::revision), which is how a harness re-emitting a message as it is
    /// written marks the draft it replaces. **Be warned**: two messages of one id that carry no
    /// revision are two messages, and a consumer upserting by id alone loses one of them (codex
    /// names a tool call and its output after one `call_id`).
    fn id(&self) -> Option<MessageId>;

    /// Which revision of [`id`](Message::id)'s message this is, where the harness re-emits one:
    /// of two messages of a session under the same id, the one with the higher revision
    /// supersedes the other.
    ///
    /// **Be warned**: revisions belong to the capture that issued them, since a harness numbers
    /// its messages within a log it is free to wipe. A session taken up again comes under
    /// revisions an earlier capture already used, so a consumer outliving one keeps the message
    /// delivered last rather than the one of the higher revision.
    ///
    /// `None`, the default, says the message is not a revision of anything: the harness writes
    /// each message once and two messages that share an id are still two messages. Revisions
    /// order messages of one session under one id and say nothing across either.
    fn revision(&self) -> Option<i64> {
        None
    }

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
    /// One model call: groups the rows a single API response is split into.
    fn turn_id(&self) -> Option<String> {
        None
    }
    /// A title this line assigns to the session.
    fn title(&self) -> Option<String> {
        None
    }
}

pub trait Session: Send + 'static {
    type Message: Message;

    fn id(&self) -> SessionId;

    /// The transcript file this session is read from.
    fn path(&self) -> &Path;

    /// Follow the transcript from byte `offset`, a boundary an earlier stream reported (past the
    /// end restarts from zero), yielding with each message the offset just past its line.
    fn messages_from(
        self,
        offset: u64,
    ) -> impl Stream<Item = Result<(u64, Self::Message), MessageError>> + Send + 'static;

    fn messages(self) -> impl Stream<Item = Result<Self::Message, MessageError>> + Send + 'static
    where
        Self: Sized,
    {
        self.messages_from(0).map_ok(|(_, message)| message)
    }

    fn read(&self) -> impl Stream<Item = Result<Self::Message, MessageError>> + Send + 'static;
}

pub trait Listener {
    type Session: Session;

    fn watch(self) -> impl Stream<Item = Result<Self::Session, WatchError>> + Send + 'static;

    /// Every line of every session the watcher reports, tagged with its session and the byte
    /// offset past it. `resume_from` is awaited once per session, with its id and transcript
    /// path, before the transcript is opened, and gives the offset to start at; 0 reads the
    /// whole file.
    fn events<F>(
        self,
        resume_from: impl Fn(&SessionId, &Path) -> F + Send + 'static,
    ) -> impl Stream<Item = Result<SessionEvent<<Self::Session as Session>::Message>, CaptureError>>
    + Send
    + 'static
    where
        Self: Sized,
        F: Future<Output = u64> + Send + 'static,
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
                            let start = resume_from(&id, session.path()).await;
                            let tag = id.clone();
                            let (tagged, handle) = futures::stream::abortable(
                                session.messages_from(start).map(move |item| (tag.clone(), item)),
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
                                Ok((offset, message)) => {
                                    yield Ok(SessionEvent { session, offset, message });
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

    fn sessions(&self) -> Self::Sessions;
}

pub mod prelude {
    pub use super::{Listener, Message, Observable, Session, Sessions};
}

#[cfg(test)]
mod tests {
    use rstest::rstest;
    use time::OffsetDateTime;

    use super::*;
    use crate::harnesstools::session::model::{Content, MessageId, Role};

    #[derive(Debug, Clone)]
    struct StubMsg {
        id: &'static str,
        revision: Option<i64>,
    }

    impl Message for StubMsg {
        fn id(&self) -> Option<MessageId> {
            Some(MessageId::from(self.id.to_owned()))
        }
        fn revision(&self) -> Option<i64> {
            self.revision
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

    fn stub(id: &'static str, revision: Option<i64>) -> StubMsg {
        StubMsg { id, revision }
    }

    fn codex(payload: &serde_json::Value) -> CodexMessage {
        serde_json::from_value(serde_json::json!({"type": "response_item", "payload": payload}))
            .unwrap()
    }

    /// What [`Message::id`] prescribes of a consumer: among the messages of one id, one
    /// supersedes another only by a higher revision, and an unrevised message supersedes
    /// nothing and is superseded by nothing.
    fn upsert<M: Message>(messages: Vec<M>) -> Vec<M> {
        let mut kept: Vec<M> = Vec::new();
        for message in messages {
            let Some(revision) = message.revision() else {
                kept.push(message);
                continue;
            };
            let superseded =
                kept.iter().position(|kept| kept.id() == message.id() && kept.revision().is_some());
            match superseded {
                Some(i) if kept[i].revision() < Some(revision) => kept[i] = message,
                Some(_) => {}
                None => kept.push(message),
            }
        }
        kept
    }

    #[rstest]
    fn message_trait_exposes_a_normalized_view() {
        let m = stub("m1", None);
        assert_eq!(m.role(), Role::Assistant);
        assert_eq!(m.content(), vec![Content::Text("hello".into())]);
        assert_eq!(m.id(), Some(MessageId::from("m1".to_owned())));
    }

    /// A transcript that yields the given offsets, then either ends or hangs like a live file
    /// waiting for more.
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

        fn path(&self) -> &Path {
            Path::new("/stub")
        }

        fn messages_from(
            self,
            _offset: u64,
        ) -> impl Stream<Item = Result<(u64, StubMsg), MessageError>> + Send + 'static {
            let items = futures::stream::iter(self.offsets.into_iter().map(|o| Ok((o, StubMsg))));
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
        let events = listener.events(|_, _| async { 0 });
        let offsets: Vec<u64> = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            events.map(|ev| ev.unwrap().offset).collect(),
        )
        .await
        .expect("the replaced stream must not keep events() alive");
        assert_eq!(offsets, vec![1, 2]);
    }

    #[rstest]
    #[case(&[("prt_1", Some(1)), ("prt_1", Some(2)), ("prt_2", Some(3))], &[Some(2), Some(3)])]
    #[case(&[("prt_1", Some(1)), ("prt_1", None), ("prt_1", Some(2))], &[Some(2), None])]
    fn a_message_supersedes_one_of_its_id_only_at_a_higher_revision(
        #[case] delivered: &[(&'static str, Option<i64>)],
        #[case] expected: &[Option<i64>],
    ) {
        let delivered = delivered.iter().map(|&(id, revision)| stub(id, revision)).collect();
        let kept: Vec<Option<i64>> = upsert(delivered).iter().map(Message::revision).collect();
        assert_eq!(kept, expected);
    }

    /// codex numbers neither half of a tool call, so the pair is indistinguishable by
    /// `(id, revision)` and an upsert keyed on that alone would drop one of them.
    #[rstest]
    fn a_codex_call_and_its_output_share_an_id_and_both_survive() {
        let call = codex(&serde_json::json!({
            "type": "function_call",
            "name": "shell",
            "arguments": "{\"cmd\":\"ls\"}",
            "call_id": "call_1",
        }));
        let output = codex(&serde_json::json!({
            "type": "function_call_output",
            "call_id": "call_1",
            "output": "files",
        }));
        assert_eq!((call.id(), call.revision()), (output.id(), output.revision()));

        let kept = upsert(vec![call, output]);
        let roles: Vec<Role> = kept.iter().map(Message::role).collect();
        assert_eq!(roles, [Role::Assistant, Role::Tool]);
        assert!(matches!(kept[0].content().as_slice(), [Content::ToolUse(_)]));
        assert!(matches!(kept[1].content().as_slice(), [Content::ToolResult(_)]));
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
