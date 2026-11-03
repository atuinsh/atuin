use std::future::Future;
use std::time::SystemTime;

use futures::stream::BoxStream;
use futures::{Stream, StreamExt, TryStreamExt};

use crate::harnesstools::ccode::session::{CcodeListener, CcodeSession, CcodeSessions};
use crate::harnesstools::codex::session::{CodexListener, CodexSession, CodexSessions};
use crate::harnesstools::copilot::session::{CopilotListener, CopilotSession, CopilotSessions};
use crate::harnesstools::grok::session::{GrokListener, GrokSession, GrokSessions};
use crate::harnesstools::opencode::session::{OpencodeListener, OpencodeSession, OpencodeSessions};
use crate::harnesstools::pi::session::{PiListener, PiSession, PiSessions};
use crate::harnesstools::session::{
    AnyMessage, CaptureError, Checkpoint, Listener, MessageError, RuntimeError, Session,
    SessionEvent, SessionId, Sessions, WatchError,
};

/// A harness's stream of results, its items converted to the `Any` type and boxed.
fn boxed<S, U, T, E>(stream: S) -> BoxStream<'static, Result<T, E>>
where
    S: Stream<Item = Result<U, E>> + Send + 'static,
    U: Into<T> + Send + 'static,
    T: Send + 'static,
    E: Send + 'static,
{
    stream.map_ok(Into::into).boxed()
}

/// A harness's result, its value converted to the `Any` type.
fn lift<U: Into<T>, T, E>(result: Result<U, E>) -> Result<T, E> {
    result.map(Into::into)
}

/// [`boxed`], for a stream behind a result.
fn boxed_existing<S, U>(
    result: Result<S, RuntimeError>,
) -> Result<BoxStream<'static, Result<AnySession, RuntimeError>>, RuntimeError>
where
    S: Stream<Item = Result<U, RuntimeError>> + Send + 'static,
    U: Into<AnySession> + Send + 'static,
{
    result.map(boxed)
}

/// [`boxed`], for a stream of events, their messages converted to [`AnyMessage`].
fn boxed_events<S, M>(
    stream: S,
) -> BoxStream<'static, Result<SessionEvent<AnyMessage>, CaptureError>>
where
    S: Stream<Item = Result<SessionEvent<M>, CaptureError>> + Send + 'static,
    M: Into<AnyMessage> + Send + 'static,
{
    stream.map_ok(|ev| ev.map_message(Into::into)).boxed()
}

#[whichever::dispatch]
#[derive(Debug, Clone)]
pub enum AnySessions {
    Ccode(CcodeSessions),
    Codex(CodexSessions),
    Copilot(CopilotSessions),
    Grok(GrokSessions),
    Opencode(OpencodeSessions),
    Pi(PiSessions),
}

#[whichever::dispatch]
impl AnySessions {
    #[dispatch(*, with = lift)]
    pub fn listener(&self) -> Result<AnyListener, RuntimeError>;

    #[dispatch(*, with = boxed_existing)]
    pub fn existing(
        &self,
    ) -> Result<BoxStream<'static, Result<AnySession, RuntimeError>>, RuntimeError>;
}

#[whichever::dispatch]
#[derive(Debug)]
pub enum AnyListener {
    Ccode(CcodeListener),
    Codex(CodexListener),
    Copilot(CopilotListener),
    Grok(GrokListener),
    Opencode(OpencodeListener),
    Pi(PiListener),
}

#[whichever::dispatch]
impl AnyListener {
    #[must_use]
    #[dispatch(*, with = boxed)]
    pub fn watch(self) -> BoxStream<'static, Result<AnySession, WatchError>>;

    /// See [`Listener::events`].
    #[must_use]
    #[dispatch(*, with = boxed_events)]
    pub fn events<F>(
        self,
        since: SystemTime,
        checkpoint: impl Fn(&SessionId) -> F + Clone + Send + 'static,
    ) -> BoxStream<'static, Result<SessionEvent<AnyMessage>, CaptureError>>
    where
        F: Future<Output = Option<Checkpoint>> + Send + 'static;
}

#[whichever::dispatch]
#[derive(Debug)]
pub enum AnySession {
    Ccode(CcodeSession),
    Codex(CodexSession),
    Copilot(CopilotSession),
    Grok(GrokSession),
    Opencode(OpencodeSession),
    Pi(PiSession),
}

#[whichever::dispatch]
impl AnySession {
    #[must_use]
    #[dispatch(*)]
    pub fn id(&self) -> SessionId;

    #[must_use]
    #[dispatch(*, with = boxed)]
    pub fn messages(self) -> BoxStream<'static, Result<AnyMessage, MessageError>>;

    #[must_use]
    #[dispatch(*, with = boxed)]
    pub fn read(&self) -> BoxStream<'static, Result<AnyMessage, MessageError>>;
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
