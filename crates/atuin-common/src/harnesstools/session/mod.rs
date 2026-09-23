pub mod error;
pub mod model;

use std::path::{Path, PathBuf};

use enum_dispatch::enum_dispatch;
pub use error::{CaptureError, MessageError, RuntimeError, WatchError};
use futures::{Stream, StreamExt};
pub use model::{
    Content, MessageId, Role, SessionEvent, SessionEventKind, SessionId, SessionMeta, StopReason,
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
    fn messages(self) -> impl Stream<Item = Result<Self::Message, MessageError>> + Send + 'static;
    fn read(&self) -> impl Stream<Item = Result<Self::Message, MessageError>> + Send + 'static;
    fn meta(&self) -> impl std::future::Future<Output = Result<SessionMeta, MessageError>> + Send {
        async { Ok(SessionMeta::default()) }
    }
}

pub trait Listener {
    type Session: Session;

    fn watch(self) -> impl Stream<Item = Result<Self::Session, WatchError>> + Send + 'static;

    fn events(
        self,
    ) -> impl Stream<Item = Result<SessionEvent<<Self::Session as Session>::Message>, CaptureError>>
    + Send
    + 'static
    where
        Self: Sized,
    {
        let sessions = self.watch();
        async_stream::stream! {
            let mut active = futures::stream::SelectAll::new();
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
                            let meta = session.meta().await.unwrap_or_default();
                            yield Ok(SessionEvent::started(id.clone(), meta));
                            let tagged =
                                session.messages().map(move |message| (id.clone(), message)).boxed();
                            active.push(tagged);
                        }
                        Some(Err(err)) => yield Err(CaptureError::from(err)),
                        None => sessions_done = true,
                    },
                    tagged = active.next(), if !active.is_empty() => {
                        if let Some((session, result)) = tagged {
                            match result {
                                Ok(message) => yield Ok(SessionEvent::message(session, message)),
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
    use rstest::rstest;
    use time::OffsetDateTime;

    use super::*;
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
}

pub mod any;

use crate::harnesstools::ccode::session::CcodeMessage;
use crate::harnesstools::codex::session::CodexMessage;
use crate::harnesstools::pi::session::PiMessage;

#[enum_dispatch(Message)]
#[derive(Debug, Clone)]
pub enum AnyMessage {
    Ccode(CcodeMessage),
    Codex(CodexMessage),
    Pi(PiMessage),
}
