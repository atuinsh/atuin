mod auto_share;
mod engine;
mod gate;
mod new_threads;
mod starts;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use atuin_client::ai_session::{AiSessionDatabase, AiSessionStore, HarnessSession, Message};
use atuin_client::settings::Settings;
use atuin_common::dirs;
use atuin_domain::record::RecordId;
use atuin_octavo::hub::{
    ConnectError, HubCallError, HubClient, HubLogin, HubSession, ThreadLookup, UserId,
};
use atuin_octavo::pb;
use atuin_octavo::queue::{QueueError, UploadQueue};
use atuin_octavo::shares::Configs;
use auto_share::AutoSharer;
use engine::{UploadEngine, UploadRetryLater, Uploaded};
use gate::{Cleared, CloseHalt, Closes, Rows};
use new_threads::{NewThread, NewThreads};
use starts::Starts;
use thiserror::Error;
use tokio::sync::Notify;
use url::Url;

use super::OctavoClient;
use crate::daemon::DaemonHandle;
use crate::octavo::{
    QueuedShares, ThreadActionError, ThreadShares, ThreadSharesError, ThreadUpload, threads_on,
};

/// How long a question about a thread the daemon hasn't heard of waits for an agent to announce
/// its start. The agent runs atuin's `SessionStart` hook beside the chip's first status read, so the
/// hook, which lands in well under its 2s limit, may come second.
const ANNOUNCED_WITHIN: Duration = Duration::from_millis(1500);

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
    new_threads: NewThreads,
    _engine: UploadEngine,
}

impl ActiveOctavoClient {
    pub async fn open(
        settings: &Settings,
        handle: DaemonHandle,
        records: AiSessionStore,
        sidecar: Option<AiSessionDatabase>,
    ) -> Result<Self, OpenActiveOctavoClientError> {
        let queue = UploadQueue::open(settings.octavo_queue_path()).await?;
        let hub = HubClient::new(settings)?;
        let wake = Arc::new(Notify::new());
        // Without a home, the user has no config of their own.
        let configs = Configs::new(dirs::try_atuin_home().ok().map(PathBuf::from));
        let starts = Starts::new(configs, sidecar, records.host_id());
        let engine = UploadEngine::spawn(
            queue.clone(),
            hub.clone(),
            handle.clone(),
            records,
            wake.clone(),
            starts.clone(),
        );

        let uploader = Arc::new(DirectUploader {
            hub,
            handle,
            queue,
            wake,
            starts,
        });

        Ok(Self {
            auto_sharer: AutoSharer::spawn(Arc::clone(&uploader)),
            uploader,
            new_threads: NewThreads::default(),
            _engine: engine,
        })
    }
}

impl ActiveOctavoClient {
    /// `session`'s shares while the user's own config turns uploading off, under the login that
    /// queued them. Without one they can't be read, and logging out closes nothing, so the thread
    /// may still be shared: `LoggedOut`, not none.
    async fn turned_off_shares(
        &self,
        session: HarnessSession,
    ) -> Result<ThreadShares, ThreadSharesError> {
        let settings = self.uploader.handle.settings().await.clone();
        let Some(hub) = self.uploader.session(&settings).await else {
            return Ok(ThreadShares::LoggedOut);
        };
        let Some(thread) = pb::HarnessSession::of_thread(session) else {
            return Ok(ThreadShares::TurnedOff(QueuedShares::default()));
        };
        let queued = self.uploader.queue.thread(hub.user_id(), &thread);
        Ok(ThreadShares::TurnedOff(QueuedShares::read(&queued).await?))
    }
}

impl OctavoClient for ActiveOctavoClient {
    async fn queue_thread_messages(&self, messages: &[Message]) -> ThreadUpload {
        let settings = self.uploader.handle.settings().await.clone();
        let messages = self.uploader.starts.uploads(messages.to_vec()).await;
        if messages.is_empty() {
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
                messages,
            });
        }

        let ids: Vec<_> = messages.iter().map(|message| message.id).collect();
        if !self.uploader.queue(session.user_id(), &ids).await {
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

    async fn thread_shares(
        &self,
        session: HarnessSession,
    ) -> Result<ThreadShares, ThreadSharesError> {
        let hub = match self.uploader.uploading().await {
            Uploading::Off => return self.turned_off_shares(session).await,
            Uploading::LoggedOut => return Ok(ThreadShares::LoggedOut),
            Uploading::As { hub, .. } => hub,
        };
        // Octavo stores no thread of a harness or session id it refuses, so none of it is shared.
        let Some(thread) = pb::HarnessSession::of_thread(session) else {
            return Ok(ThreadShares::On(QueuedShares::default()));
        };
        let queued = self.uploader.queue.thread(hub.user_id(), &thread);
        Ok(ThreadShares::On(QueuedShares::read(&queued).await?))
    }

    async fn make_private(
        &self,
        session: HarnessSession,
    ) -> Result<ThreadShares, ThreadSharesError> {
        let hub = match self.uploader.uploading().await {
            // Closes and revocations wait while uploading is off, so nothing changes.
            Uploading::Off => return Ok(ThreadShares::TurnedOff(QueuedShares::default())),
            Uploading::LoggedOut => return Ok(ThreadShares::LoggedOut),
            Uploading::As { hub, .. } => hub,
        };
        // The host that captured a thread queues its shares, so this queue has none to close.
        if self.uploader.starts.captured_elsewhere(&session).await {
            return Err(ThreadSharesError::CapturedElsewhere);
        }
        let thread =
            pb::HarnessSession::of_thread(session).ok_or(ThreadSharesError::NotShareable)?;
        let queued = self.uploader.queue.thread(hub.user_id(), &thread);
        queued.make_private().await?;
        self.uploader.wake.notify_one();
        Ok(ThreadShares::On(QueuedShares::read(&queued).await?))
    }

    async fn share_link(&self, session: HarnessSession) -> Result<Url, ThreadActionError> {
        // The host that captured a thread holds its private mark, so this one can't tell whether
        // the user unshared it.
        if self.uploader.starts.captured_elsewhere(&session).await {
            return Err(ThreadActionError::CapturedElsewhere);
        }
        let OnOctavo {
            settings,
            login,
            thread,
            names,
        } = self.uploader.on_octavo(session).await?;
        let queued = self.uploader.queue.thread(login.user_id(), &names);
        if queued.sharing().await?.is_private {
            return Err(ThreadActionError::Private);
        }
        let id = thread.id.ok_or(HubCallError::Incomplete("thread id"))?;
        let link = login.thread_link(&id).await?;
        // An unshare may have come in, and its revocation gone, while the hub was asked: the link
        // is then one nothing would delete. Queued again, the revocation deletes it. An unshare
        // that comes in after this read revokes the link itself. Only a daemon that dies between
        // the hub's answer and this write leaves a private thread its link.
        if queued.sharing().await?.is_private {
            queued.make_private().await?;
            self.uploader.wake.notify_one();
            return Err(ThreadActionError::Private);
        }
        Ok(link.url(&settings.hub_endpoint())?)
    }

    async fn add_labels(
        &self,
        session: HarnessSession,
        names: &[String],
    ) -> Result<Vec<String>, ThreadActionError> {
        let OnOctavo { login, thread, .. } = self.uploader.on_octavo(session).await?;
        Ok(login.add_labels(&thread, names).await?)
    }

    fn thread_started(&self, session: HarnessSession, dir: PathBuf) {
        self.new_threads.add(NewThread { session, dir });
    }

    async fn new_thread_shares(&self, id: &str) -> Option<ThreadShares> {
        let thread = self.new_threads.wait_for(id, ANNOUNCED_WITHIN).await?;
        Some(match self.uploader.uploading().await {
            // A thread that hasn't started never went up, so nothing of it is on Atuin Hub.
            Uploading::Off => ThreadShares::TurnedOff(QueuedShares::default()),
            Uploading::LoggedOut => ThreadShares::LoggedOut,
            Uploading::As { .. } => {
                ThreadShares::On(thread.shares(self.uploader.starts.configs()).await)
            }
        })
    }
}

/// Current Octavo upload state and active hub session.
enum Uploading {
    Off,
    LoggedOut,
    As {
        settings: Arc<Settings>,
        hub: HubSession,
    },
}

/// A thread stored on Octavo and a login as its owner.
struct OnOctavo {
    settings: Arc<Settings>,
    login: HubLogin,
    thread: pb::Thread,
    /// The thread by its harness's names, as the upload queue knows it.
    names: pb::HarnessSession,
}

/// Queues thread messages for the engine, and uploads those that skip the queue.
#[derive(Debug)]
struct DirectUploader {
    hub: HubClient,
    handle: DaemonHandle,
    queue: UploadQueue,
    wake: Arc<Notify>,
    /// Who threads are shared with, and which threads go up.
    starts: Starts,
}

impl DirectUploader {
    /// Clears the mark [`mark_off`] left on `user`'s account, as an upload just went through, and
    /// wakes the engine to send what the mark paused, if there was one.
    async fn resume(&self, user: &UserId) {
        if clear_off(&self.queue, user).await {
            self.wake.notify_one();
        }
    }

    /// Queues `user`'s thread messages `ids` for the engine to send before returning, so they
    /// outlive the daemon, and wakes the engine.
    async fn enqueue(&self, user: &UserId, ids: &[RecordId]) {
        if self.queue(user, ids).await {
            self.wake.notify_one();
        }
    }

    /// Queues `user`'s thread messages `ids` for the engine to send before returning, so they
    /// outlive the daemon, without waking the engine. Whether they were queued: a failure is
    /// logged and the messages dropped.
    async fn queue(&self, user: &UserId, ids: &[RecordId]) -> bool {
        if let Err(err) = self.queue.push_thread_messages(user, ids.iter().copied()).await {
            tracing::warn!(
                ?err,
                messages = ids.len(),
                "failed to queue thread messages for octavo; they will not reach it"
            );
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
        let closing = rows.record_dirs(self.starts.configs()).await?.narrow().await?;
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

    async fn uploading(&self) -> Uploading {
        if !threads_on(self.starts.configs()).await {
            return Uploading::Off;
        }
        let settings = self.handle.settings().await.clone();
        match self.session(&settings).await {
            Some(hub) => Uploading::As { settings, hub },
            None => Uploading::LoggedOut,
        }
    }

    /// Find `session`'s thread on Octavo and log in as the active user.
    async fn on_octavo(&self, session: HarnessSession) -> Result<OnOctavo, ThreadActionError> {
        let (settings, hub) = match self.uploading().await {
            Uploading::Off => return Err(ThreadActionError::Off),
            Uploading::LoggedOut => return Err(ThreadActionError::LoggedOut),
            Uploading::As { settings, hub } => (settings, hub),
        };
        let names =
            pb::HarnessSession::of_thread(session).ok_or(ThreadActionError::NotShareable)?;
        let login = self.hub.login_as(hub, &settings).map_err(ThreadActionError::Login)?;
        let thread = match login.find_thread(&names).await? {
            ThreadLookup::Found(thread) => *thread,
            ThreadLookup::NotStored => return Err(ThreadActionError::NotStored),
            ThreadLookup::NotFoundYet(_) => return Err(ThreadActionError::NotAmongRecent),
        };
        Ok(OnOctavo {
            settings,
            login,
            thread,
            names,
        })
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

        match engine::upload_thread_messages(&uploader.queue, &login, messages).await {
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
                uploader.enqueue(user, &ids).await;
            }
        }
    }
}

/// Marks `user`'s account as having Octavo off, which pauses their queue, and keeps their new
/// thread messages out of it, until an upload finds Octavo on again.
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
