mod client;

use std::sync::Arc;

use atuin_client::history::{History, HistoryId};
use atuin_client::settings::Settings;
use atuin_domain::record::RecordId;
use client::{AnyOctavoClient, NopOctavoClient, OctavoClient};

use crate::daemon::DaemonHandle;
use crate::output_capture::OutputStore;

#[derive(Debug)]
pub struct Octavo {
    client: AnyOctavoClient,
}

impl Octavo {
    #[cfg(feature = "octavo")]
    pub async fn open(
        settings: &Settings,
        handle: DaemonHandle,
        outputs: Arc<OutputStore>,
    ) -> Self {
        // Once Octavo has been on here, deletions keep reaching it with uploads off, so what the
        // user deletes locally leaves Octavo too.
        if !settings.octavo.enabled && !Settings::octavo_queue_path().exists() {
            return Self::nop();
        }

        match client::ActiveOctavoClient::open(settings, handle, outputs).await {
            Ok(client) => Self {
                client: client.into(),
            },
            Err(err) => {
                tracing::error!(?err, "failed to open octavo; octavo uploads are disabled");
                Self::nop()
            }
        }
    }

    #[cfg(not(feature = "octavo"))]
    #[allow(clippy::unused_async)]
    pub async fn open(
        _settings: &Settings,
        _handle: DaemonHandle,
        _outputs: Arc<OutputStore>,
    ) -> Self {
        Self::nop()
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
}
