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

use super::{ThreadShares, ThreadSharesError, ThreadUpload, UserId};

#[enum_dispatch]
#[allow(async_fn_in_trait)]
pub trait OctavoClient {
    async fn queue_thread_messages(&self, messages: &[Message]) -> ThreadUpload;
    fn consider_sharing(&self, user: UserId, session: HarnessSession, sidecar: &AiSessionDatabase);
    async fn thread_shares(
        &self,
        session: HarnessSession,
    ) -> Result<ThreadShares, ThreadSharesError>;
}

#[enum_dispatch(OctavoClient)]
#[derive(Debug)]
pub enum AnyOctavoClient {
    Active(ActiveOctavoClient),
    Nop(NopOctavoClient),
    #[cfg(test)]
    Recording(RecordingOctavoClient),
}
