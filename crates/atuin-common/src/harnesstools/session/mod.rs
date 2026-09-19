pub mod error;
pub mod model;

use enum_dispatch::enum_dispatch;
pub use error::{CaptureError, MessageError, RuntimeError, WatchError};
use futures::{Stream, StreamExt};
pub use model::{
    Content, MessageId, Role, SessionEvent, SessionEventKind, SessionId, SessionMeta, StopReason,
    ToolCallId, ToolResult, ToolUse, Usage,
};
use time::OffsetDateTime;

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
}

pub trait Session: Send + 'static {
    type Message: Message;

    fn id(&self) -> SessionId;
    fn messages(self) -> impl Stream<Item = Result<Self::Message, MessageError>> + Send + 'static;
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
