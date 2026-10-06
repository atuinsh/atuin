#[cfg(feature = "octavo")]
mod active;
mod nop;

#[cfg(feature = "octavo")]
pub use active::ActiveOctavoClient;
use atuin_client::history::{History, HistoryId};
use atuin_domain::record::RecordId;
use enum_dispatch::enum_dispatch;
pub use nop::NopOctavoClient;

#[enum_dispatch]
#[allow(async_fn_in_trait)]
pub trait OctavoClient {
    async fn push_history(&self, history: &History, record_id: RecordId);
    async fn push_output(&self, id: HistoryId);
    async fn delete_history(&self, ids: &[HistoryId]);
}

#[enum_dispatch(OctavoClient)]
#[derive(Debug)]
pub enum AnyOctavoClient {
    #[cfg(feature = "octavo")]
    Active(ActiveOctavoClient),
    Nop(NopOctavoClient),
}
