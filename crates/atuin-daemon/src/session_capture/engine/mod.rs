mod actor;
mod listener;

use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

use atuin_client::ai_session::HarnessKind;
use atuin_common::harnesstools::AnyHarness;
use atuin_common::harnesstools::session::{Listener, Observable, Sessions};
use listener::HarnessListener;
use tokio::task::JoinHandle;

use super::Sink;

pub(crate) struct SessionCaptureEngine {
    _listeners: Vec<JoinHandle<()>>,
    #[cfg(test)]
    followers: Arc<AtomicUsize>,
}

impl SessionCaptureEngine {
    pub(crate) fn detached() -> Self {
        Self {
            _listeners: Vec::new(),
            #[cfg(test)]
            followers: Arc::new(AtomicUsize::new(0)),
        }
    }

    pub(crate) fn spawn(sink: Arc<Sink>) -> Self {
        let followers = Arc::new(AtomicUsize::new(0));
        let mut listeners = Vec::new();

        for harness in AnyHarness::all() {
            Self::spawn_listener(*harness, &sink, &followers, &mut listeners);
        }

        Self {
            _listeners: listeners,
            #[cfg(test)]
            followers,
        }
    }

    #[cfg(test)]
    pub(crate) fn spawn_with<O>(sink: Arc<Sink>, harnesses: Vec<O>) -> Self
    where
        O: Observable,
    {
        let followers = Arc::new(AtomicUsize::new(0));
        let mut listeners = Vec::new();

        for harness in harnesses {
            Self::spawn_listener(harness, &sink, &followers, &mut listeners);
        }

        Self {
            _listeners: listeners,
            followers,
        }
    }

    fn spawn_listener<O>(
        harness: O,
        sink: &Arc<Sink>,
        followers: &Arc<AtomicUsize>,
        listeners: &mut Vec<JoinHandle<()>>,
    ) where
        O: Observable,
    {
        let Some(sessions) = harness.sessions() else {
            return;
        };
        let Ok(listener) = sessions.listener() else {
            return;
        };

        let watch = listener.watch();
        let listener = HarnessListener::new(
            HarnessKind::from(harness.kind()),
            sink.clone(),
            followers.clone(),
            watch,
        );
        listeners.push(tokio::spawn(listener.run()));
    }

    #[cfg(test)]
    pub(crate) fn live_follower_count(&self) -> usize {
        self.followers.load(std::sync::atomic::Ordering::SeqCst)
    }
}

impl Drop for SessionCaptureEngine {
    fn drop(&mut self) {
        for listener in &self._listeners {
            listener.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::sync::Arc;
    use std::time::Duration;

    use atuin_client::ai_session::{
        AiSessionDatabase, AiSessionStore, HarnessKind, HarnessSession, NativeSessionId,
    };
    use atuin_client::record::sqlite_store::SqliteStore;
    use atuin_common::encryption::paseto_v4::Key;
    use atuin_common::harnesstools::session::ReadFrom;
    use atuin_domain::record::{HostId, RecordTag};
    use futures::StreamExt;
    use rstest::rstest;

    use super::SessionCaptureEngine;
    use crate::session_capture::testkit::{ScriptedHarness, assistant, err, ok, user};
    use crate::session_capture::{SessionTailEvent, Sink};

    const STORE_TIMEOUT: Duration = Duration::from_secs(5);

    async fn mem_store() -> AiSessionStore {
        let store = SqliteStore::in_memory(STORE_TIMEOUT).await.unwrap();
        AiSessionStore::builder()
            .store(store)
            .host_id(HostId(atuin_common::utils::uuid_v7()))
            .key(Key::generate())
            .build()
    }

    fn handle(id: &str) -> HarnessSession {
        HarnessSession {
            harness: HarnessKind::Codex,
            session: NativeSessionId::from(id.to_owned()),
        }
    }

    async fn wait_for<F, Fut>(mut condition: F)
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = bool>,
    {
        for _ in 0..2_000 {
            if condition().await {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("condition was not satisfied before the timeout elapsed");
    }

    async fn drive_once(store: &AiSessionStore, db: &AiSessionDatabase, harness: ScriptedHarness) {
        let sink = Arc::new(Sink::new(store.clone(), db.clone()));
        let _engine = SessionCaptureEngine::spawn_with(sink, vec![harness]);
        wait_for(|| async {
            matches!(
                db.checkpoint(HarnessKind::Codex, &"s".to_string().into()).await.unwrap(),
                ReadFrom::Offset(offset) if offset >= 2
            )
        })
        .await;
    }

    #[rstest]
    #[tokio::test]
    async fn captures_scripted_session_into_sidecar_and_tail() {
        let db = AiSessionDatabase::in_memory().await.unwrap();
        let sink = Arc::new(Sink::new(mem_store().await, db.clone()));
        let mut sub = sink.tail_subscribe();

        let harness = ScriptedHarness::appeared(handle("s1"), vec![user("hi"), assistant("yo")]);
        let _engine = SessionCaptureEngine::spawn_with(sink.clone(), vec![harness]);

        wait_for(|| async {
            db.get_session(&handle("s1")).await.unwrap().map(|s| s.message_count) == Some(2)
        })
        .await;
        assert!(matches!(sub.next().await.unwrap().unwrap(), SessionTailEvent::SessionStarted(_)));
    }

    #[rstest]
    #[tokio::test]
    async fn dormant_existing_session_is_backfilled_without_a_follower() {
        let db = AiSessionDatabase::in_memory().await.unwrap();
        let sink = Arc::new(Sink::new(mem_store().await, db.clone()));
        let harness = ScriptedHarness::existing_dormant(handle("old"), vec![user("done")]);
        let engine = SessionCaptureEngine::spawn_with(sink.clone(), vec![harness]);

        wait_for(|| async { db.get_session(&handle("old")).await.unwrap().is_some() }).await;
        assert_eq!(engine.live_follower_count(), 0);
    }

    #[rstest]
    #[tokio::test]
    async fn restart_from_checkpoint_produces_no_duplicate_records() {
        let store = mem_store().await;
        let db = AiSessionDatabase::in_memory().await.unwrap();

        drive_once(
            &store,
            &db,
            ScriptedHarness::appeared(handle("s"), vec![user("a"), assistant("b")]),
        )
        .await;
        drive_once(
            &store,
            &db,
            ScriptedHarness::appeared(handle("s"), vec![user("a"), assistant("b")]),
        )
        .await;

        assert_eq!(store.len_tag(&RecordTag::AiSession).await.unwrap(), 2);
    }

    #[rstest]
    #[tokio::test]
    async fn malformed_message_advances_checkpoint_and_is_skipped() {
        let db = AiSessionDatabase::in_memory().await.unwrap();
        let sink = Arc::new(Sink::new(mem_store().await, db.clone()));
        let harness = ScriptedHarness::appeared_with_offsets(handle("s"), vec![
            (2, ok(user("a"))),
            (4, err()),
            (6, ok(assistant("b"))),
        ]);
        let _engine = SessionCaptureEngine::spawn_with(sink.clone(), vec![harness]);

        wait_for(|| async {
            db.get_session(&handle("s")).await.unwrap().map(|s| s.message_count) == Some(2)
                && matches!(
                    db.checkpoint(HarnessKind::Codex, &"s".to_string().into()).await.unwrap(),
                    ReadFrom::Offset(6)
                )
        })
        .await;
        assert!(matches!(
            db.checkpoint(HarnessKind::Codex, &"s".to_string().into()).await.unwrap(),
            ReadFrom::Offset(6)
        ));
    }
}
