use atuin_client::history::{History, HistoryId};
use atuin_domain::record::RecordId;

use super::OctavoClient;

#[derive(Debug, Clone, Copy)]
pub struct NopOctavoClient;

impl OctavoClient for NopOctavoClient {
    async fn push_history(&self, _history: &History, _record_id: RecordId) {}

    async fn delete_history(&self, _ids: &[HistoryId]) {}
}
