use std::sync::Mutex;

use atuin_client::ai_session::HarnessSession;
use atuin_common::harnesstools::session::{
    Appearance, Content, HarnessKind as HHarness, Listener, Message as HMessage, MessageError,
    MessageId, Observable, ReadFrom, Role, RuntimeError, Session as HSession, SessionId,
    SessionMeta, Sessions, StopReason, Usage, WatchError,
};
use futures::stream::{self, BoxStream, StreamExt};
use time::OffsetDateTime;

type Script = Vec<(u64, Result<ScriptedMessage, MessageError>)>;

struct Recording {
    existing: bool,
    handle: HarnessSession,
    meta: SessionMeta,
    script: Script,
}

pub(crate) struct ScriptedHarness {
    recording: Mutex<Option<Recording>>,
}

pub(crate) struct ScriptedSessions {
    recording: Mutex<Option<Recording>>,
}

pub(crate) struct ScriptedListener {
    recording: Recording,
}

pub(crate) struct ScriptedSession {
    handle: HarnessSession,
    meta: SessionMeta,
    script: Script,
}

#[derive(Clone)]
pub(crate) struct ScriptedMessage {
    role: Role,
    content: Vec<Content>,
}

impl ScriptedHarness {
    pub(crate) fn appeared(handle: HarnessSession, messages: Vec<ScriptedMessage>) -> Self {
        Self::from_recording(false, handle, Self::recent_meta(), Self::sequential(messages))
    }

    pub(crate) fn existing_dormant(handle: HarnessSession, messages: Vec<ScriptedMessage>) -> Self {
        Self::from_recording(true, handle, Self::dormant_meta(), Self::sequential(messages))
    }

    pub(crate) fn appeared_with_offsets(handle: HarnessSession, script: Script) -> Self {
        Self::from_recording(false, handle, Self::recent_meta(), script)
    }

    fn from_recording(
        existing: bool,
        handle: HarnessSession,
        meta: SessionMeta,
        script: Script,
    ) -> Self {
        Self {
            recording: Mutex::new(Some(Recording {
                existing,
                handle,
                meta,
                script,
            })),
        }
    }

    fn sequential(messages: Vec<ScriptedMessage>) -> Script {
        messages
            .into_iter()
            .enumerate()
            .map(|(index, message)| (index as u64 + 1, Ok(message)))
            .collect()
    }

    fn recent_meta() -> SessionMeta {
        SessionMeta {
            cwd: None,
            git_branch: None,
            model: None,
            started_at: OffsetDateTime::now_utc(),
            title: None,
            parent: None,
        }
    }

    fn dormant_meta() -> SessionMeta {
        SessionMeta {
            started_at: OffsetDateTime::now_utc() - time::Duration::days(1),
            ..Self::recent_meta()
        }
    }
}

impl Observable for ScriptedHarness {
    type Sessions = ScriptedSessions;

    fn kind(&self) -> HHarness {
        HHarness::Codex
    }

    fn sessions(&self) -> Option<Self::Sessions> {
        self.recording.lock().expect("scripted harness lock").take().map(|recording| {
            ScriptedSessions {
                recording: Mutex::new(Some(recording)),
            }
        })
    }
}

impl Sessions for ScriptedSessions {
    type Listener = ScriptedListener;

    fn listener(&self) -> Result<Self::Listener, RuntimeError> {
        let recording = self
            .recording
            .lock()
            .expect("scripted sessions lock")
            .take()
            .expect("scripted sessions consumed exactly once");
        Ok(ScriptedListener { recording })
    }
}

impl Listener for ScriptedListener {
    type Session = ScriptedSession;

    fn watch(self) -> BoxStream<'static, Result<Appearance<Self::Session>, WatchError>> {
        let Recording {
            existing,
            handle,
            meta,
            script,
        } = self.recording;
        let session = ScriptedSession {
            handle,
            meta,
            script,
        };
        let appearance = if existing {
            Appearance::Existing(session)
        } else {
            Appearance::Appeared(session)
        };
        stream::once(async move { Ok(appearance) }).chain(stream::pending()).boxed()
    }
}

impl ScriptedSession {
    fn remaining(script: Script, from: ReadFrom) -> Script {
        let cutoff = match from {
            ReadFrom::Beginning => 0,
            ReadFrom::Offset(offset) => offset,
        };
        script.into_iter().filter(|(offset, _)| *offset > cutoff).collect()
    }
}

impl HSession for ScriptedSession {
    type Message = ScriptedMessage;

    fn id(&self) -> SessionId {
        SessionId::from(self.handle.session.as_ref().to_owned())
    }

    fn messages_from(
        self,
        from: ReadFrom,
    ) -> BoxStream<'static, (u64, Result<Self::Message, MessageError>)> {
        stream::iter(Self::remaining(self.script, from)).chain(stream::pending()).boxed()
    }

    fn messages_once_from(
        self,
        from: ReadFrom,
    ) -> BoxStream<'static, (u64, Result<Self::Message, MessageError>)> {
        stream::iter(Self::remaining(self.script, from)).boxed()
    }

    async fn meta(&self) -> Result<SessionMeta, MessageError> {
        Ok(self.meta.clone())
    }

    async fn usage_total(&self) -> Result<Option<Usage>, MessageError> {
        Ok(None)
    }
}

impl HMessage for ScriptedMessage {
    fn id(&self) -> Option<MessageId> {
        None
    }

    fn role(&self) -> Role {
        self.role.clone()
    }

    fn timestamp(&self) -> Option<OffsetDateTime> {
        None
    }

    fn content(&self) -> Vec<Content> {
        self.content.clone()
    }

    fn model(&self) -> Option<String> {
        None
    }

    fn usage(&self) -> Option<Usage> {
        None
    }

    fn stop_reason(&self) -> Option<StopReason> {
        None
    }
}

pub(crate) fn user(text: &str) -> ScriptedMessage {
    ScriptedMessage {
        role: Role::User,
        content: vec![Content::Text(text.to_owned())],
    }
}

pub(crate) fn assistant(text: &str) -> ScriptedMessage {
    ScriptedMessage {
        role: Role::Assistant,
        content: vec![Content::Text(text.to_owned())],
    }
}

pub(crate) fn ok(message: ScriptedMessage) -> Result<ScriptedMessage, MessageError> {
    Ok(message)
}

pub(crate) fn err() -> Result<ScriptedMessage, MessageError> {
    Err(MessageError::Parse("scripted malformed message".to_owned()))
}
