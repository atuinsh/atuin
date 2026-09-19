mod actor;
mod listener;

use std::sync::Arc;

use atuin_client::ai_session::HarnessKind;
use atuin_common::harnesstools::AnyHarness;
use atuin_common::harnesstools::session::{Listener, Observable, Sessions};
use listener::HarnessListener;
use tokio::task::JoinHandle;

use super::Sink;

pub(crate) struct SessionCaptureEngine {
    _listeners: Vec<JoinHandle<()>>,
}

impl SessionCaptureEngine {
    pub(crate) fn detached() -> Self {
        Self {
            _listeners: Vec::new(),
        }
    }

    pub(crate) fn spawn(sink: Arc<Sink>) -> Self {
        let mut listeners = Vec::new();

        for harness in AnyHarness::all() {
            Self::spawn_listener(*harness, &sink, &mut listeners);
        }

        Self {
            _listeners: listeners,
        }
    }

    fn spawn_listener<O>(harness: O, sink: &Arc<Sink>, listeners: &mut Vec<JoinHandle<()>>)
    where
        O: Observable,
    {
        let Some(sessions) = harness.sessions() else {
            return;
        };
        let Ok(listener) = sessions.listener() else {
            return;
        };

        let watch = listener.watch();
        let listener = HarnessListener::new(HarnessKind::from(harness.kind()), sink.clone(), watch);
        listeners.push(tokio::spawn(listener.run()));
    }
}

impl Drop for SessionCaptureEngine {
    fn drop(&mut self) {
        for listener in &self._listeners {
            listener.abort();
        }
    }
}
