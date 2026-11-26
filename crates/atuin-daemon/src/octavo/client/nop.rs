use atuin_client::ai_session::{AiSessionDatabase, HarnessSession, Message};
use url::Url;

use super::OctavoClient;
use crate::octavo::{ThreadActionError, ThreadShares, ThreadSharesError, ThreadUpload, UserId};

#[derive(Debug, Clone, Copy)]
pub struct NopOctavoClient;

impl OctavoClient for NopOctavoClient {
    async fn queue_thread_messages(&self, _messages: &[Message]) -> ThreadUpload {
        ThreadUpload::Nothing
    }

    fn consider_sharing(
        &self,
        _user: UserId,
        _session: HarnessSession,
        _sidecar: &AiSessionDatabase,
    ) {
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

    async fn share_link(&self, _session: HarnessSession) -> Result<Url, ThreadActionError> {
        Err(ThreadActionError::Off)
    }

    async fn add_labels(
        &self,
        _session: HarnessSession,
        _names: &[String],
    ) -> Result<Vec<String>, ThreadActionError> {
        Err(ThreadActionError::Off)
    }
}
