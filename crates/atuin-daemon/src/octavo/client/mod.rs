mod active;
mod nop;
#[cfg(test)]
mod recording;

pub use active::{ActiveOctavoClient, DirectThreadUpload};
use atuin_client::ai_session::{AiSessionDatabase, HarnessSession, Message};
use enum_dispatch::enum_dispatch;
pub use nop::NopOctavoClient;
#[cfg(test)]
pub use recording::RecordingOctavoClient;
use url::Url;

use super::{ThreadActionError, ThreadShares, ThreadSharesError, ThreadUpload, UserId};

#[enum_dispatch]
#[allow(async_fn_in_trait)]
pub trait OctavoClient {
    async fn queue_thread_messages(&self, messages: &[Message]) -> ThreadUpload;
    fn consider_sharing(&self, user: UserId, session: HarnessSession, sidecar: &AiSessionDatabase);
    async fn thread_shares(
        &self,
        session: HarnessSession,
    ) -> Result<ThreadShares, ThreadSharesError>;
    async fn make_private(
        &self,
        session: HarnessSession,
    ) -> Result<ThreadShares, ThreadSharesError>;
    async fn share_link(&self, session: HarnessSession) -> Result<Url, ThreadActionError>;
    async fn add_labels(
        &self,
        session: HarnessSession,
        names: &[String],
    ) -> Result<Vec<String>, ThreadActionError>;
}

#[enum_dispatch(OctavoClient)]
#[derive(Debug)]
pub enum AnyOctavoClient {
    Active(ActiveOctavoClient),
    Nop(NopOctavoClient),
    #[cfg(test)]
    Recording(RecordingOctavoClient),
}
