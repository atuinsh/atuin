mod active;
mod nop;
#[cfg(test)]
mod recording;

pub use active::{ActiveOctavoClient, DirectThreadUpload};
use atuin_client::ai_session::{AiSessionDatabase, HarnessSession, Message};
use atuin_client::history::{History, HistoryId};
use atuin_domain::record::RecordId;
use enum_dispatch::enum_dispatch;
pub use nop::NopOctavoClient;
#[cfg(test)]
pub use recording::RecordingOctavoClient;

use super::{ThreadUpload, UserId};

#[enum_dispatch]
#[allow(async_fn_in_trait)]
pub trait OctavoClient {
    async fn push_history(&self, history: &History, record_id: RecordId);
    async fn push_output(&self, id: HistoryId);
    async fn delete_history(&self, ids: &[HistoryId]);
    async fn queue_thread_messages(&self, messages: &[Message]) -> ThreadUpload;
    fn consider_sharing(&self, user: UserId, session: HarnessSession, sidecar: &AiSessionDatabase);
}

#[enum_dispatch(OctavoClient)]
#[derive(Debug)]
pub enum AnyOctavoClient {
    Active(ActiveOctavoClient),
    Nop(NopOctavoClient),
    #[cfg(test)]
    Recording(RecordingOctavoClient),
}
