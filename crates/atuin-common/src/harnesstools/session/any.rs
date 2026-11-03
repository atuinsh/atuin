use std::future::Future;
use std::time::SystemTime;

use futures::stream::BoxStream;
use futures::{StreamExt, TryStreamExt};

use crate::harnesstools::ccode::session::{CcodeListener, CcodeSession, CcodeSessions};
use crate::harnesstools::codex::session::{CodexListener, CodexSession, CodexSessions};
use crate::harnesstools::copilot::session::{CopilotListener, CopilotSession, CopilotSessions};
use crate::harnesstools::grok::session::{GrokListener, GrokSession, GrokSessions};
use crate::harnesstools::opencode::session::{OpencodeListener, OpencodeSession, OpencodeSessions};
use crate::harnesstools::pi::session::{PiListener, PiSession, PiSessions};
use crate::harnesstools::session::{
    AnyMessage, CaptureError, Checkpoint, Listener, MessageError, Observable, RuntimeError,
    Session, SessionEvent, SessionId, Sessions, WatchError,
};
use crate::sync::BlockingPool;

/// Runtime-polymorphic version of [`Observable`]: returns "Any" types instead of concrete
/// per-harness types. Implemented by [`AnyHarness`](enum@crate::harnesstools::AnyHarness).
#[whichever::dispatchable]
pub trait ObservableExt {
    /// `Any`-returning version of [`Observable::sessions`].
    fn sessions(&self, pool: &BlockingPool) -> Option<AnySessions>;
}

impl<H> ObservableExt for H
where
    H: Observable,
    H::Sessions: Into<AnySessions>,
{
    fn sessions(&self, pool: &BlockingPool) -> Option<AnySessions> {
        Some(Observable::sessions(self, pool.clone()).into())
    }
}

/// Runtime-polymorphic version of [`Sessions`]: returns "Any" types instead of concrete
/// per-harness types. Implemented by [`AnySessions`](enum@AnySessions).
#[whichever::dispatchable]
pub trait SessionsExt {
    /// `Any`-returning version of [`Sessions::listener`].
    fn listener(&self) -> Result<AnyListener, RuntimeError>;

    /// `Any`-returning version of [`Sessions::existing`].
    fn existing(
        &self,
    ) -> Result<BoxStream<'static, Result<AnySession, RuntimeError>>, RuntimeError>;
}

impl<S> SessionsExt for S
where
    S: Sessions,
    S::Listener: Into<AnyListener>,
    <S::Listener as Listener>::Session: Into<AnySession>,
{
    fn listener(&self) -> Result<AnyListener, RuntimeError> {
        Sessions::listener(self).map(Into::into)
    }

    fn existing(
        &self,
    ) -> Result<BoxStream<'static, Result<AnySession, RuntimeError>>, RuntimeError> {
        Ok(Sessions::existing(self)?.map_ok(Into::into).boxed())
    }
}

#[whichever::dispatch(SessionsExt)]
#[derive(Debug, Clone)]
pub enum AnySessions {
    Ccode(CcodeSessions),
    Codex(CodexSessions),
    Copilot(CopilotSessions),
    Grok(GrokSessions),
    Opencode(OpencodeSessions),
    Pi(PiSessions),
}

/// Runtime-polymorphic version of [`Listener`]: returns "Any" types instead of concrete
/// per-harness types. Implemented by [`AnyListener`](enum@AnyListener).
#[whichever::dispatchable]
pub trait ListenerExt {
    /// `Any`-returning version of [`Listener::watch`].
    fn watch(self) -> BoxStream<'static, Result<AnySession, WatchError>>;

    /// `Any`-returning version of [`Listener::events`].
    fn events<F>(
        self,
        since: SystemTime,
        checkpoint: impl Fn(&SessionId) -> F + Clone + Send + 'static,
    ) -> BoxStream<'static, Result<SessionEvent<AnyMessage>, CaptureError>>
    where
        F: Future<Output = Option<Checkpoint>> + Send + 'static;
}

impl<L> ListenerExt for L
where
    L: Listener,
    L::Session: Into<AnySession>,
    <L::Session as Session>::Message: Into<AnyMessage>,
{
    fn watch(self) -> BoxStream<'static, Result<AnySession, WatchError>> {
        Listener::watch(self).map_ok(Into::into).boxed()
    }

    fn events<F>(
        self,
        since: SystemTime,
        checkpoint: impl Fn(&SessionId) -> F + Clone + Send + 'static,
    ) -> BoxStream<'static, Result<SessionEvent<AnyMessage>, CaptureError>>
    where
        F: Future<Output = Option<Checkpoint>> + Send + 'static,
    {
        Listener::events(self, since, checkpoint).map_ok(|ev| ev.map_message(Into::into)).boxed()
    }
}

#[whichever::dispatch(ListenerExt)]
#[derive(Debug)]
pub enum AnyListener {
    Ccode(CcodeListener),
    Codex(CodexListener),
    Copilot(CopilotListener),
    Grok(GrokListener),
    Opencode(OpencodeListener),
    Pi(PiListener),
}

/// Runtime-polymorphic version of [`Session`]: returns "Any" types instead of concrete
/// per-harness types. Implemented by [`AnySession`](enum@AnySession).
#[whichever::dispatchable]
pub trait SessionExt {
    /// Same as [`Session::id`]; in this trait so [`AnySessions`](enum@AnySessions) can implement
    /// it.
    fn id(&self) -> SessionId;

    /// `Any`-returning version of [`Session::messages`].
    fn messages(self) -> BoxStream<'static, Result<AnyMessage, MessageError>>;

    /// `Any`-returning version of [`Session::read`].
    fn read(&self) -> BoxStream<'static, Result<AnyMessage, MessageError>>;
}

impl<S> SessionExt for S
where
    S: Session,
    S::Message: Into<AnyMessage>,
{
    fn id(&self) -> SessionId {
        Session::id(self)
    }

    fn messages(self) -> BoxStream<'static, Result<AnyMessage, MessageError>> {
        Session::messages(self).map_ok(Into::into).boxed()
    }

    fn read(&self) -> BoxStream<'static, Result<AnyMessage, MessageError>> {
        Session::read(self).map_ok(Into::into).boxed()
    }
}

#[whichever::dispatch(SessionExt)]
#[derive(Debug)]
pub enum AnySession {
    Ccode(CcodeSession),
    Codex(CodexSession),
    Copilot(CopilotSession),
    Grok(GrokSession),
    Opencode(OpencodeSession),
    Pi(PiSession),
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use rstest::rstest;

    use super::*;
    use crate::sync::BlockingPool;

    fn pool() -> BlockingPool {
        BlockingPool::new(std::num::NonZeroUsize::MIN)
    }
    use crate::harnesstools::ccode::session::CcodeMessage;
    use crate::harnesstools::session::Message;
    use crate::harnesstools::session::model::Role;

    #[rstest]
    fn any_sessions_listener_reports_not_found() {
        let sessions = AnySessions::from(
            CcodeSessions::builder().root(PathBuf::from("/no/such")).pool(pool()).build(),
        );
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
