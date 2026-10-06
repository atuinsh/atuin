mod engine;

use std::sync::Arc;

use atuin_client::history::{History, HistoryId};
use atuin_client::settings::Settings;
use atuin_domain::record::RecordId;
use atuin_octavo::hub::{ConnectError, HubCallError, HubClient, HubToken, UserId};
use atuin_octavo::pb;
use atuin_octavo::queue::{QueueError, UploadQueue};
use engine::UploadEngine;
use thiserror::Error;
use tokio::sync::Notify;

use super::OctavoClient;
use crate::daemon::DaemonHandle;

#[derive(Debug, Error)]
pub enum OpenActiveOctavoClientError {
    #[error("failed to open the upload queue")]
    Queue(#[from] QueueError),
    #[error("failed to set up the hub client")]
    Hub(#[from] ConnectError),
}

#[derive(Debug)]
pub struct ActiveOctavoClient {
    uploader: Arc<DirectUploader>,
    _engine: UploadEngine,
}

impl ActiveOctavoClient {
    pub async fn open(
        settings: &Settings,
        handle: DaemonHandle,
    ) -> Result<Self, OpenActiveOctavoClientError> {
        let queue = UploadQueue::open(Settings::octavo_queue_path()).await?;
        let hub = HubClient::new(settings)?;
        let wake = Arc::new(Notify::new());
        let engine = UploadEngine::spawn(queue.clone(), hub.clone(), handle.clone(), wake.clone());

        Ok(Self {
            uploader: Arc::new(DirectUploader {
                hub,
                handle,
                queue,
                wake,
            }),
            _engine: engine,
        })
    }
}

impl OctavoClient for ActiveOctavoClient {
    async fn push_history(&self, history: &History, record_id: RecordId) {
        let settings = self.uploader.handle.settings().await.clone();
        if !settings.octavo.enabled {
            return;
        }
        // Read now, so the command goes to whoever was logged in as it finished.
        let Some(token) = self.uploader.token(&settings).await else {
            return;
        };

        // Detached, so finishing a command never waits on the network. A daemon that exits while
        // the upload is in flight loses only that entry's upload.
        tokio::spawn(Arc::clone(&self.uploader).upload_or_queue(
            settings,
            token,
            history.clone(),
            record_id,
        ));
    }

    async fn delete_history(&self, ids: &[HistoryId]) {
        let settings = self.uploader.handle.settings().await.clone();
        // Read now, so the deletions go to whoever was logged in when the entries were deleted.
        let Some(token) = self.uploader.token(&settings).await else {
            return;
        };

        // Queued before the local delete returns, so the deletion outlives the daemon. That needs
        // the token's user, known once the daemon has reached the hub since the login; until
        // then a detached task asks the hub first. The engine sends them once they're queued.
        match self.uploader.hub.user_of(&token).await {
            Ok(Some(user)) => self.uploader.queue_deletions(&user, ids).await,
            Ok(None) => {
                tokio::spawn(Arc::clone(&self.uploader).learn_user_and_queue_deletions(
                    settings,
                    token,
                    ids.to_vec(),
                ));
            }
            Err(err) => {
                tracing::warn!(
                    ?err,
                    count = ids.len(),
                    "failed to read the hub login; these deletions will not reach octavo"
                );
            }
        }
    }
}

/// Uploads a finished command as soon as it finishes, leaving the queue to the ones that can't go
/// right away.
#[derive(Debug)]
struct DirectUploader {
    hub: HubClient,
    handle: DaemonHandle,
    queue: UploadQueue,
    wake: Arc<Notify>,
}

impl DirectUploader {
    /// Uploads `history`, or queues it for the engine to retry when the upload fails for a reason
    /// that may pass, such as no network.
    async fn upload_or_queue(
        self: Arc<Self>,
        settings: Settings,
        token: HubToken,
        history: History,
        record_id: RecordId,
    ) {
        let id = history.id;
        let login = match self.hub.login_as(token, &settings).await {
            Ok(login) => login,
            Err(err) => {
                tracing::warn!(
                    ?err,
                    %id,
                    "failed to learn whose hub token this is; not uploading this history entry"
                );
                return;
            }
        };

        match login.upload(history, Some(record_id)).await {
            Ok(()) => {}
            Err(err) if err.is_permanent() => {
                tracing::warn!(
                    ?err,
                    %id,
                    "octavo will never accept this history entry; dropping it"
                );
            }
            Err(err) => {
                tracing::debug!(
                    ?err,
                    %id,
                    "failed to upload a history entry to octavo; queueing it"
                );
                if let Err(err) = self.queue.push_history(login.user_id(), id, record_id).await {
                    tracing::warn!(
                        ?err,
                        %id,
                        "failed to queue a history entry for octavo; it will not be uploaded"
                    );
                    return;
                }
                self.wake.notify_one();
            }
        }
    }

    /// Learns whose `token` is from the hub, then queues the deletion of `ids` for them.
    async fn learn_user_and_queue_deletions(
        self: Arc<Self>,
        settings: Settings,
        token: HubToken,
        ids: Vec<HistoryId>,
    ) {
        match self.hub.login_as(token, &settings).await {
            Ok(login) => self.queue_deletions(login.user_id(), &ids).await,
            Err(err) => {
                tracing::warn!(
                    ?err,
                    count = ids.len(),
                    "failed to learn whose hub token this is; these deletions will not reach \
                     octavo"
                );
            }
        }
    }

    /// Queues the deletion of `user`'s entries `ids` for the engine to send.
    async fn queue_deletions(&self, user: &UserId, ids: &[HistoryId]) {
        // Octavo never stored an entry whose id isn't a UUIDv7.
        let stored = ids.iter().copied().filter(|&id| pb::UuidV7::try_from(id).is_ok());
        if let Err(err) = self.queue.push_deletions(user, stored).await {
            tracing::warn!(
                ?err,
                count = ids.len(),
                "failed to queue history deletions for octavo; they will not reach it"
            );
            return;
        }

        self.wake.notify_one();
    }

    /// The hub token logged in now, or `None` with Octavo left alone when there's none to use.
    async fn token(&self, settings: &Settings) -> Option<HubToken> {
        match self.hub.token(settings).await {
            Ok(token) => Some(token),
            Err(HubCallError::NotLoggedIn) => None,
            Err(err) => {
                tracing::warn!(?err, "failed to read the hub session; leaving octavo alone");
                None
            }
        }
    }
}
