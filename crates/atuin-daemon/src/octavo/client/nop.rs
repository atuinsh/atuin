use atuin_client::ai_session::Message;
use atuin_client::history::{History, HistoryId};
use atuin_domain::record::RecordId;

use super::OctavoClient;
use crate::octavo::ThreadUpload;

#[derive(Debug, Clone, Copy)]
pub struct NopOctavoClient;

impl OctavoClient for NopOctavoClient {
    async fn push_history(&self, _history: &History, _record_id: RecordId) {}

    async fn push_output(&self, _id: HistoryId) {}

    async fn delete_history(&self, _ids: &[HistoryId]) {}

    async fn queue_thread_messages(&self, _messages: &[Message]) -> ThreadUpload {
        ThreadUpload::Nothing
    }
}
