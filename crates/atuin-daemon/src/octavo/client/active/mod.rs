mod auto_share;
mod engine;
mod gate;

use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

use atuin_client::ai_session::{AiSessionDatabase, AiSessionStore, HarnessSession, Message};
use atuin_client::history::{History, HistoryId};
use atuin_client::settings::Settings;
use atuin_common::dirs;
use atuin_domain::record::RecordId;
use atuin_octavo::hub::{ConnectError, HubCallError, HubClient, HubLogin, HubSession, UserId};
use atuin_octavo::pb;
use atuin_octavo::queue::{QueueError, UploadQueue};
use atuin_octavo::shares::Configs;
use auto_share::AutoSharer;
use engine::{UploadEngine, UploadRetryLater, Uploaded};
use gate::{Cleared, CloseHalt, Closes, Rows};
use thiserror::Error;
use tokio::sync::Notify;

use super::OctavoClient;
use crate::daemon::DaemonHandle;
use crate::octavo::ThreadUpload;
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
    auto_sharer: AutoSharer,
    _engine: UploadEngine,
}

impl ActiveOctavoClient {
    pub async fn open(
        settings: &Settings,
        handle: DaemonHandle,
        outputs: Arc<OutputStore>,
        records: AiSessionStore,
    ) -> Result<Self, OpenActiveOctavoClientError> {
        let queue = UploadQueue::open(settings.octavo_queue_path()).await?;
        let hub = HubClient::new(settings)?;
        let wake = Arc::new(Notify::new());
        // Without a home, the user has no config of their own.
        let configs = Configs::new(dirs::try_atuin_home().ok().map(PathBuf::from));
        let engine = UploadEngine::spawn(
            queue.clone(),
            hub.clone(),
            handle.clone(),
            outputs,
            records,
            wake.clone(),
            configs.clone(),
        );

        let uploader = Arc::new(DirectUploader {
            hub,
            handle,
            queue,
            wake,
            configs,
        });

        Ok(Self {
            auto_sharer: AutoSharer::spawn(Arc::clone(&uploader)),
            uploader,
            _engine: engine,
        })
    }
}

impl OctavoClient for ActiveOctavoClient {
    async fn push_history(&self, history: &History, record_id: RecordId) {
        let settings = self.uploader.handle.settings().await.clone();
        if !settings.teams.enabled {
            return;
        }
        // Read now, so the command goes to whoever was logged in as it finished.
        let Some(session) = self.uploader.session(&settings).await else {
            return;
        };

        // Detached, so finishing a command never waits on the network. A daemon that exits while
        // the upload is in flight loses only that entry's upload.
        tokio::spawn(Arc::clone(&self.uploader).upload_or_queue(
            settings,
            session,
            history.clone(),
            record_id,
        ));
    }

    async fn push_output(&self, id: HistoryId) {
        let settings = self.uploader.handle.settings().await.clone();
        if !(settings.teams.enabled && settings.teams.upload_output) {
            return;
        }
        // Read now, so the output goes to whoever was logged in as it was captured.
        let Some(session) = self.uploader.session(&settings).await else {
            return;
        };
        // Queued even while the account is marked off, which pauses the queue, to wait on its
        // entry's direct upload: a success sends it, and a refusal, or a failure while the mark
        // holds, forgets it. One stored after a refusal waits out the off spell, then expires
        // unsent, as Octavo has no entry for it.
        self.uploader.enqueue(session.user_id(), Queued::Output(id)).await;
    }

    async fn delete_history(&self, ids: &[HistoryId]) {
        let settings = self.uploader.handle.settings().await.clone();
        // Read now, so the deletions go to whoever was logged in when the entries were deleted.
        let Some(session) = self.uploader.session(&settings).await else {
            return;
        };

        self.uploader.enqueue(session.user_id(), Queued::Deletions(ids.to_vec())).await;
    }

    async fn queue_thread_messages(&self, messages: &[Message]) -> ThreadUpload {
        let settings = self.uploader.handle.settings().await.clone();
        if !settings.teams.enabled {
            return ThreadUpload::Nothing;
        }
        // Read now, so the messages go to whoever was logged in as they were captured.
        let Some(session) = self.uploader.session(&settings).await else {
            return ThreadUpload::Nothing;
        };
        // The mark trails the account's Octavo both ways:
        // - From the switch-off until the first refusal lands, messages are queued. That is
        //   normally at most one debounce. They wait in the queue with the rest, and go up once
        //   Octavo is back on.
        // - A refusal sent before a re-enable can land after a success sent after it, which marks
        //   the account off, and pauses its queue, until the next success: at the latest the
        //   engine's probe of the queue, a poll later.
        // Sent direct on a stale mark, the messages still go up, and clear it. Should that fail
        // for a reason that may pass, they're dropped while it holds.
        if is_off(&self.uploader.queue, session.user_id()).await {
            return ThreadUpload::Direct(DirectThreadUpload {
                uploader: Arc::clone(&self.uploader),
                settings,
                session,
                messages: messages.to_vec(),
            });
        }

        let ids = messages.iter().map(|message| message.id).collect();
        if !self.uploader.queue(session.user_id(), Queued::ThreadMessages(ids)).await {
            return ThreadUpload::Nothing;
        }
        ThreadUpload::Queued {
            wake: self.uploader.wake.clone(),
            user: session.user_id().clone(),
        }
    }

    fn consider_sharing(&self, user: UserId, session: HarnessSession, sidecar: &AiSessionDatabase) {
        self.auto_sharer.consider(user, session, sidecar);
    }
}

/// Work for the engine to send, queued under the user a hub token belongs to.
#[derive(Debug)]
enum Queued {
    Deletions(Vec<HistoryId>),
    Output(HistoryId),
    ThreadMessages(Vec<RecordId>),
}

impl fmt::Display for Queued {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Deletions(ids) => write!(f, "{} deletions", ids.len()),
            Self::Output(id) => write!(f, "the output of {id}"),
            Self::ThreadMessages(ids) => write!(f, "{} thread messages", ids.len()),
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
    /// Who threads are shared with.
    configs: Configs,
}

impl DirectUploader {
    /// Uploads `history`, or queues it for the engine to retry when the upload fails for a reason
    /// that may pass, such as no network, and the account isn't marked off.
    async fn upload_or_queue(
        self: Arc<Self>,
        settings: Arc<Settings>,
        session: HubSession,
        history: History,
        record_id: RecordId,
    ) {
        let id = history.id;
        let login = match self.hub.login_as(session, &settings) {
            Ok(login) => login,
            Err(err) => {
                tracing::warn!(
                    ?err,
                    %id,
                    "failed to log in to octavo; not uploading this history entry"
                );
                return;
            }
        };

        let entry = match pb::History::with_record(history, Some(record_id)) {
            Ok(entry) => entry,
            Err(err) => {
                tracing::warn!(
                    ?err,
                    %id,
                    "octavo will never accept this history entry; dropping it"
                );
                forget_output(&self.queue, login.user_id(), id).await;
                return;
            }
        };

        match login.upload_history(pb::HistoryBatch::one(entry)).await {
            Ok(()) => {
                self.resume(login.user_id()).await;
                // The entry's output may be queued already, waiting on it.
                if settings.teams.upload_output {
                    self.wake.notify_one();
                }
            }
            Err(HubCallError::OctavoOff(_)) => {
                tracing::debug!(%id, "octavo is off for the account; dropping this history entry");
                mark_off(&self.queue, login.user_id()).await;
                forget_output(&self.queue, login.user_id(), id).await;
            }
            Err(err) if err.is_permanent() => {
                tracing::warn!(
                    ?err,
                    %id,
                    "octavo will never accept this history entry; dropping it"
                );
                forget_output(&self.queue, login.user_id(), id).await;
            }
            Err(err) => {
                // Work captured while the account has Octavo off is never queued: it would go up
                // once Octavo is back.
                if is_off(&self.queue, login.user_id()).await {
                    tracing::debug!(
                        ?err,
                        %id,
                        "failed to upload a history entry to octavo; dropping it, as the account is \
                         marked off"
                    );
                    forget_output(&self.queue, login.user_id(), id).await;
                    return;
                }
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

    /// Clears the mark [`mark_off`] left on `user`'s account, as an upload just went through, and
    /// wakes the engine to send what the mark paused, if there was one.
    async fn resume(&self, user: &UserId) {
        if clear_off(&self.queue, user).await {
            self.wake.notify_one();
        }
    }

    /// Queues `user`'s `work` for the engine to send before returning, so it outlives the daemon,
    /// and wakes the engine.
    async fn enqueue(&self, user: &UserId, work: Queued) {
        if self.queue(user, work).await {
            self.wake.notify_one();
        }
    }

    /// Queues `user`'s `work` for the engine to send before returning, so it outlives the daemon,
    /// without waking the engine. Whether it was queued: a failure is logged and the work dropped.
    async fn queue(&self, user: &UserId, work: Queued) -> bool {
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
            Queued::ThreadMessages(ids) => {
                self.queue.push_thread_messages(user, ids.iter().copied()).await.map(drop)
            }
        };
        if let Err(err) = queued {
            tracing::warn!(?err, %work, "failed to queue for octavo; it will not reach it");
            return false;
        }
        true
    }

    /// Those of `messages` that may go up now, cleared as the engine clears them (see
    /// [`gate`]). The rest are dropped, as messages captured while the account is marked off get
    /// one try.
    async fn clear(
        &self,
        login: &HubLogin,
        messages: Vec<Message>,
    ) -> Result<Vec<Message>, CloseHalt> {
        let rows = Rows::new(&self.queue, login.user_id(), messages);
        let closing = rows.record_dirs(&self.configs).await?.narrow().await?;
        let deadline = tokio::time::Instant::now() + engine::PASS_BUDGET;
        let sent = Closes::of(&self.queue, login).send(deadline).await?;
        let Cleared { ready, waiting } = closing.clear(sent).await?;
        if !waiting.is_empty() {
            tracing::debug!(
                dropped = waiting.len(),
                "not uploading thread messages whose threads can't be cleared now"
            );
        }
        Ok(ready)
    }

    /// The hub session logged in now, or `None` with Octavo left alone when there's none to use.
    async fn session(&self, settings: &Settings) -> Option<HubSession> {
        match self.hub.session(settings).await {
            Ok(session) => Some(session),
            Err(HubCallError::NotLoggedIn) => None,
            Err(HubCallError::UnknownUser) => {
                // A session saved before atuin stored its user. Until the engine learns the user,
                // Octavo gets nothing: work queued under no user could reach whoever logs in next.
                tracing::debug!("the hub login's user is not known yet; leaving octavo alone");
                self.wake.notify_one();
                None
            }
            Err(err) => {
                tracing::warn!(?err, "failed to read the hub session; leaving octavo alone");
                None
            }
        }
    }
}

/// Thread messages to upload once, directly, rather than queue, as the account had Octavo off
/// when last asked: queued, they would wait out the off spell and go up after it.
#[derive(Debug)]
pub struct DirectThreadUpload {
    uploader: Arc<DirectUploader>,
    settings: Arc<Settings>,
    session: HubSession,
    messages: Vec<Message>,
}

impl DirectThreadUpload {
    pub fn user(&self) -> &UserId {
        self.session.user_id()
    }

    /// Uploads the messages, whose records are stored, without waiting on the network.
    ///
    /// A daemon that exits while the upload is in flight loses only these messages' upload.
    pub fn start(self) {
        tokio::spawn(self.upload());
    }

    async fn upload(self) {
        let Self {
            uploader,
            settings,
            session,
            messages,
        } = self;
        let login = match uploader.hub.login_as(session, &settings) {
            Ok(login) => login,
            Err(err) => {
                tracing::warn!(?err, "failed to log in to octavo; not uploading these messages");
                return;
            }
        };
        let user = login.user_id();
        let ready = match uploader.clear(&login, messages).await {
            Ok(ready) => ready,
            Err(CloseHalt::Off) => return,
            Err(CloseHalt::Queue(err)) => {
                tracing::warn!(?err, "failed to read the octavo share queue; not uploading these");
                return;
            }
        };
        let ids: Vec<_> = ready.iter().map(|message| message.id).collect();
        let messages = engine::converted(ready).await;

        match engine::upload_thread_messages(&login, messages).await {
            Ok(Uploaded::Stored) => {
                uploader.resume(user).await;
            }
            Ok(Uploaded::NoneStored | Uploaded::Nothing) => {}
            Ok(Uploaded::Off) => {
                tracing::debug!("octavo is off for the account; dropping these thread messages");
                mark_off(&uploader.queue, user).await;
            }
            Err(UploadRetryLater { stored }) => {
                // A stored request shows the account has Octavo on, so the mark is stale and the
                // rest goes up again; the stored ones repeat, which Octavo dedupes.
                if stored {
                    uploader.resume(user).await;
                } else if is_off(&uploader.queue, user).await {
                    tracing::debug!(
                        "failed to upload thread messages to octavo; dropping them, as the \
                         account is marked off"
                    );
                    return;
                }
                uploader.enqueue(user, Queued::ThreadMessages(ids)).await;
            }
        }
    }
}

/// Marks `user`'s account as having Octavo off, which pauses their queue, and keeps their new
/// entries and thread messages out of it, until an upload finds Octavo on again.
async fn mark_off(queue: &UploadQueue, user: &UserId) {
    match queue.mark_off(user).await {
        Ok(true) => {
            tracing::info!("octavo is off for the account; pausing its uploads until it's on");
        }
        Ok(false) => {}
        Err(err) => tracing::warn!(?err, "failed to note that octavo is off for the account"),
    }
}

/// Clears the mark [`mark_off`] left on `user`'s account, as an upload just found Octavo on,
/// returning whether there was one.
async fn clear_off(queue: &UploadQueue, user: &UserId) -> bool {
    match queue.clear_off(user).await {
        Ok(true) => {
            tracing::info!("octavo is on for the account again");
            true
        }
        Ok(false) => false,
        Err(err) => {
            tracing::warn!(?err, "failed to note that octavo is on for the account");
            false
        }
    }
}

/// Whether `user`'s account had Octavo off when last asked. One the queue can't say for is taken
/// to have it: its work is queued and sent.
async fn is_off(queue: &UploadQueue, user: &UserId) -> bool {
    queue.is_off(user).await.unwrap_or_else(|err| {
        tracing::warn!(?err, "failed to read whether octavo is off for the account");
        false
    })
}

/// Removes the queued output of `user`'s entry `id`, which Octavo didn't store.
async fn forget_output(queue: &UploadQueue, user: &UserId, id: HistoryId) {
    if let Err(err) = queue.remove_output(user, id).await {
        tracing::warn!(
            ?err,
            %id,
            "failed to remove the output of a dropped history entry from the queue"
        );
    }
}
