mod client;

use std::sync::Arc;

use atuin_client::ai_session::{AiSessionStore, Message};
use atuin_client::history::{History, HistoryId};
use atuin_client::settings::Settings;
use atuin_domain::record::RecordId;
use client::{AnyOctavoClient, NopOctavoClient, OctavoClient};
use tokio::sync::Notify;

use crate::daemon::DaemonHandle;
use crate::output_capture::OutputStore;

/// Thread messages queued for Octavo before capture stores their records: they go once
/// [`Self::start`] says the records are stored, so no upload reads the queue ahead of them.
#[must_use = "the queued messages wait for a later upload unless started"]
#[derive(Debug, Default)]
pub struct ThreadUpload {
    wake: Option<Arc<Notify>>,
}

impl ThreadUpload {
    /// Starts the upload, the messages' records stored.
    pub fn start(self) {
        if let Some(wake) = self.wake {
            wake.notify_one();
        }
    }
}

#[derive(Debug)]
pub struct Octavo {
    client: AnyOctavoClient,
}

impl Octavo {
    /// Opens the client for `settings`, reading queued outputs from `outputs` and queued thread
    /// messages from `records`.
    pub async fn open(
        settings: &Settings,
        handle: DaemonHandle,
        outputs: Arc<OutputStore>,
        records: AiSessionStore,
    ) -> Self {
        // Once Octavo has been on here, deletions keep reaching it with uploads off, so what the
        // user deletes locally leaves Octavo too.
        if !settings.octavo.enabled && !settings.octavo_queue_path().exists() {
            return Self::nop();
        }

        match client::ActiveOctavoClient::open(settings, handle, outputs, records).await {
            Ok(client) => Self {
                client: client.into(),
            },
            Err(err) => {
                tracing::error!(?err, "failed to open octavo; octavo uploads are disabled");
                Self::nop()
            }
        }
    }

    #[must_use]
    pub fn nop() -> Self {
        Self {
            client: NopOctavoClient.into(),
        }
    }

    pub async fn push_history(&self, history: &History, record_id: RecordId) {
        self.client.push_history(history, record_id).await;
    }

    /// Queues the upload of the output captured for `id`, which goes once its entry is stored.
    pub async fn push_output(&self, id: HistoryId) {
        self.client.push_output(id).await;
    }

    pub async fn delete_history(&self, ids: &[HistoryId]) {
        self.client.delete_history(ids).await;
    }

    /// Queues the upload of `messages`, captured on this host, which goes once their records are
    /// stored and the returned [`ThreadUpload`] started.
    pub async fn queue_thread_messages(&self, messages: &[Message]) -> ThreadUpload {
        self.client.queue_thread_messages(messages).await
    }
}
