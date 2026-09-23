use std::future::Future;
use std::path::Path;

use derive_more::From;
use futures::stream::BoxStream;
use futures::{StreamExt, TryStreamExt};

use crate::harnesstools::ccode::session::{CcodeListener, CcodeSession, CcodeSessions};
use crate::harnesstools::codex::session::{CodexListener, CodexSession, CodexSessions};
use crate::harnesstools::pi::session::{PiListener, PiSession, PiSessions};
use crate::harnesstools::session::{
    AnyMessage, CaptureError, Listener, MessageError, RuntimeError, Session, SessionEvent,
    SessionId, Sessions, WatchError,
};

#[derive(Debug, Clone, From)]
pub enum AnySessions {
    Ccode(CcodeSessions),
    Codex(CodexSessions),
    Pi(PiSessions),
}

impl AnySessions {
    pub fn listener(&self) -> Result<AnyListener, RuntimeError> {
        Ok(match self {
            Self::Ccode(s) => AnyListener::Ccode(s.listener()?),
            Self::Codex(s) => AnyListener::Codex(s.listener()?),
            Self::Pi(s) => AnyListener::Pi(s.listener()?),
        })
    }

    pub fn existing(
        &self,
    ) -> Result<BoxStream<'static, Result<AnySession, RuntimeError>>, RuntimeError> {
        Ok(match self {
            Self::Ccode(s) => s.existing()?.map_ok(AnySession::from).boxed(),
            Self::Codex(s) => s.existing()?.map_ok(AnySession::from).boxed(),
            Self::Pi(s) => s.existing()?.map_ok(AnySession::from).boxed(),
        })
    }
}

#[derive(Debug, From)]
pub enum AnyListener {
    Ccode(CcodeListener),
    Codex(CodexListener),
    Pi(PiListener),
}

impl AnyListener {
    #[must_use]
    pub fn watch(self) -> BoxStream<'static, Result<AnySession, WatchError>> {
        match self {
            Self::Ccode(l) => l.watch().map_ok(AnySession::from).boxed(),
            Self::Codex(l) => l.watch().map_ok(AnySession::from).boxed(),
            Self::Pi(l) => l.watch().map_ok(AnySession::from).boxed(),
        }
    }

    /// See [`Listener::events`].
    #[must_use]
    pub fn events<F>(
        self,
        resume_from: impl Fn(&SessionId, &Path) -> F + Send + 'static,
    ) -> BoxStream<'static, Result<SessionEvent<AnyMessage>, CaptureError>>
    where
        F: Future<Output = u64> + Send + 'static,
    {
        match self {
            Self::Ccode(l) => {
                l.events(resume_from).map_ok(|ev| ev.map_message(AnyMessage::from)).boxed()
            }
            Self::Codex(l) => {
                l.events(resume_from).map_ok(|ev| ev.map_message(AnyMessage::from)).boxed()
            }
            Self::Pi(l) => {
                l.events(resume_from).map_ok(|ev| ev.map_message(AnyMessage::from)).boxed()
            }
        }
    }
}

#[derive(Debug, From)]
pub enum AnySession {
    Ccode(CcodeSession),
    Codex(CodexSession),
    Pi(PiSession),
}

impl AnySession {
    #[must_use]
    pub fn id(&self) -> SessionId {
        match self {
            Self::Ccode(s) => s.id(),
            Self::Codex(s) => s.id(),
            Self::Pi(s) => s.id(),
        }
    }

    #[must_use]
    pub fn messages(self) -> BoxStream<'static, Result<AnyMessage, MessageError>> {
        match self {
            Self::Ccode(s) => s.messages().map_ok(AnyMessage::from).boxed(),
            Self::Codex(s) => s.messages().map_ok(AnyMessage::from).boxed(),
            Self::Pi(s) => s.messages().map_ok(AnyMessage::from).boxed(),
        }
    }

    #[must_use]
    pub fn read(&self) -> BoxStream<'static, Result<AnyMessage, MessageError>> {
        match self {
            Self::Ccode(s) => s.read().map_ok(AnyMessage::from).boxed(),
            Self::Codex(s) => s.read().map_ok(AnyMessage::from).boxed(),
            Self::Pi(s) => s.read().map_ok(AnyMessage::from).boxed(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use rstest::rstest;

    use super::*;
    use crate::harnesstools::ccode::session::CcodeMessage;
    use crate::harnesstools::session::Message;
    use crate::harnesstools::session::model::Role;

    #[rstest]
    fn any_sessions_listener_reports_not_found() {
        let sessions =
            AnySessions::from(CcodeSessions::builder().root(PathBuf::from("/no/such")).build());
        assert!(matches!(sessions.listener(), Err(RuntimeError::NotFound(_))));
    }

    #[rstest]
    fn any_message_dispatches_the_message_trait() {
        let raw = serde_json::json!({
            "type": "assistant",
            "message": {"role": "assistant", "content": "hi"},
        })
        .to_string();
        let concrete: CcodeMessage = serde_json::from_str(&raw).unwrap();
        let any = AnyMessage::from(concrete);
        assert_eq!(any.role(), Role::Assistant);
    }
}
