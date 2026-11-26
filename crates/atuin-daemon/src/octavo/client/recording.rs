use std::sync::Arc;

use atuin_client::ai_session::{AiSessionDatabase, HarnessSession, Message};
use parking_lot::Mutex;

use super::OctavoClient;
use crate::octavo::{ThreadShares, ThreadSharesError, ThreadUpload, UserId};

/// Queues thread messages under [`Self::USER`], uploading none, and remembers the threads it was
/// asked to consider sharing.
#[derive(Debug, Clone, Default)]
pub struct RecordingOctavoClient {
    considered: Arc<Mutex<Vec<(UserId, HarnessSession)>>>,
}

impl RecordingOctavoClient {
    pub const USER: &str = "recorded";

    pub fn considered(&self) -> Vec<(UserId, HarnessSession)> {
        self.considered.lock().clone()
    }
}

impl OctavoClient for RecordingOctavoClient {
    async fn queue_thread_messages(&self, _messages: &[Message]) -> ThreadUpload {
        ThreadUpload::Queued {
            wake: Arc::default(),
            user: UserId::new(Self::USER),
        }
    }

    fn consider_sharing(
        &self,
        user: UserId,
        session: HarnessSession,
        _sidecar: &AiSessionDatabase,
    ) {
        self.considered.lock().push((user, session));
    }

    async fn thread_shares(
        &self,
        _session: HarnessSession,
    ) -> Result<ThreadShares, ThreadSharesError> {
        Ok(ThreadShares::Off)
    }

    async fn make_private(
        &self,
        _session: HarnessSession,
    ) -> Result<ThreadShares, ThreadSharesError> {
        Ok(ThreadShares::Off)
    }
}
