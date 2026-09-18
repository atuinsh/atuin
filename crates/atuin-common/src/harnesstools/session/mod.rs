pub mod error;
pub mod model;

use enum_dispatch::enum_dispatch;
pub use error::{MessageError, RuntimeError, WatchError};
use futures::Stream;
pub use model::{Content, MessageId, Role, SessionId, ToolCallId, ToolResult, ToolUse};
use time::OffsetDateTime;

#[enum_dispatch]
pub trait Message {
    fn id(&self) -> Option<MessageId>;
    fn role(&self) -> Role;
    fn timestamp(&self) -> Option<OffsetDateTime>;
    fn content(&self) -> Vec<Content>;
}

pub trait Session {
    type Message: Message;

    fn id(&self) -> SessionId;
    fn messages(self) -> impl Stream<Item = Result<Self::Message, MessageError>> + Send + 'static;
}

pub trait Listener {
    type Session: Session;

    fn watch(self) -> impl Stream<Item = Result<Self::Session, WatchError>> + Send + 'static;
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
