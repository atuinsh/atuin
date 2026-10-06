mod engine;

use std::fmt;
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
use crate::output_capture::OutputStore;

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
        outputs: Arc<OutputStore>,
    ) -> Result<Self, OpenActiveOctavoClientError> {
        let queue = UploadQueue::open(Settings::octavo_queue_path()).await?;
        let hub = HubClient::new(settings)?;
        let wake = Arc::new(Notify::new());
        let engine =
            UploadEngine::spawn(queue.clone(), hub.clone(), handle.clone(), outputs, wake.clone());

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

    async fn push_output(&self, id: HistoryId) {
        let settings = self.uploader.handle.settings().await.clone();
        if !(settings.octavo.enabled && settings.octavo.upload_output) {
            return;
        }
        // Read now, so the output goes to whoever was logged in as it was captured.
        let Some(token) = self.uploader.token(&settings).await else {
            return;
        };

        self.uploader.queue_for(settings, token, Queued::Output(id)).await;
    }

    async fn delete_history(&self, ids: &[HistoryId]) {
        let settings = self.uploader.handle.settings().await.clone();
        // Read now, so the deletions go to whoever was logged in when the entries were deleted.
        let Some(token) = self.uploader.token(&settings).await else {
            return;
        };

        self.uploader.queue_for(settings, token, Queued::Deletions(ids.to_vec())).await;
    }
}

/// Work for the engine to send, queued under the user a hub token belongs to.
#[derive(Debug)]
enum Queued {
    Deletions(Vec<HistoryId>),
    Output(HistoryId),
}

impl fmt::Display for Queued {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Deletions(ids) => write!(f, "{} deletions", ids.len()),
            Self::Output(id) => write!(f, "the output of {id}"),
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
            // The entry's output may be queued already, waiting on it.
            Ok(()) if settings.octavo.upload_output => self.wake.notify_one(),
            Ok(()) => {}
            Err(err) if err.is_permanent() => {
                tracing::warn!(
                    ?err,
                    %id,
                    "octavo will never accept this history entry; dropping it"
                );
                if let Err(err) = self.queue.remove_output(login.user_id(), id).await {
                    tracing::warn!(
                        ?err,
                        %id,
                        "failed to remove the output of a refused history entry from the queue"
                    );
                }
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

    /// Queues `work` for `token`'s user before returning, so it outlives the daemon.
    ///
    /// That needs the token's user, known once the daemon has reached the hub since the login;
    /// until then a detached task asks the hub first.
    async fn queue_for(self: &Arc<Self>, settings: Settings, token: HubToken, work: Queued) {
        match self.hub.user_of(&token).await {
            Ok(Some(user)) => self.enqueue(&user, work).await,
            Ok(None) => {
                tokio::spawn(Arc::clone(self).learn_user_and_queue(settings, token, work));
            }
            Err(err) => {
                tracing::warn!(
                    ?err,
                    %work,
                    "failed to read the hub login; not queueing for octavo"
                );
            }
        }
    }

    /// Learns whose `token` is from the hub, then queues `work` for them.
    async fn learn_user_and_queue(
        self: Arc<Self>,
        settings: Settings,
        token: HubToken,
        work: Queued,
    ) {
        match self.hub.login_as(token, &settings).await {
            Ok(login) => self.enqueue(login.user_id(), work).await,
            Err(err) => {
                tracing::warn!(
                    ?err,
                    %work,
                    "failed to learn whose hub token this is; not queueing for octavo"
                );
            }
        }
    }

    /// Queues `user`'s `work` for the engine to send, and wakes it.
    async fn enqueue(&self, user: &UserId, work: Queued) {
        let queued = match &work {
            // Octavo never stored an entry whose id isn't a UUIDv7.
            Queued::Deletions(ids) => self
                .queue
                .push_deletions(
                    user,
                    ids.iter().copied().filter(|&id| pb::UuidV7::try_from(id).is_ok()),
                )
                .await
                .map(drop),
            Queued::Output(id) => self.queue.push_output(user, *id).await,
        };
        if let Err(err) = queued {
            tracing::warn!(?err, %work, "failed to queue for octavo; it will not reach it");
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
