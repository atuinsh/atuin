mod client;

use std::error::Error as StdError;
use std::path::PathBuf;
use std::sync::Arc;

use atuin_client::ai_session::{AiSessionDatabase, AiSessionStore, HarnessSession, Message};
use atuin_client::settings::Settings;
use atuin_common::dirs;
use atuin_octavo::hub::HubCallError;
pub use atuin_octavo::hub::UserId;
use atuin_octavo::queue::{DirShares, OrgShare, QueueError, QueuedThread, ThreadSharing};
use atuin_octavo::shares::Configs;
#[cfg(test)]
pub use client::RecordingOctavoClient;
use client::{AnyOctavoClient, DirectThreadUpload, NopOctavoClient, OctavoClient};
use thiserror::Error;
use tokio::sync::Notify;
use url::Url;

use crate::daemon::DaemonHandle;

/// Octavo sharing state for a thread from the local upload queue.
#[derive(Debug)]
pub enum ThreadShares {
    /// Octavo is disabled.
    Off,
    /// Octavo has run on this machine, and the user's own config has since turned uploading
    /// off: no message goes up, but what the thread was shared with before stays shared.
    TurnedOff(QueuedShares),
    /// No user is logged in to Atuin Hub.
    LoggedOut,
    On(QueuedShares),
}

/// Queued sharing state for a thread under the active user.
#[derive(Debug, Default)]
pub struct QueuedShares {
    /// Organizations the thread is shared with and their share state.
    pub orgs: Vec<OrgShare>,
    /// Directories the thread uploaded messages from and each directory's configured organizations.
    pub dirs: Vec<DirShares>,
    pub is_private: bool,
    /// Whether a private thread's share links are still queued to be revoked.
    pub revoking_links: bool,
}

impl QueuedShares {
    async fn read(thread: &QueuedThread<'_>) -> Result<Self, QueueError> {
        let ThreadSharing {
            orgs,
            dirs,
            is_private,
            revoking_links,
        } = thread.sharing().await?;
        Ok(Self {
            orgs,
            dirs,
            is_private,
            revoking_links,
        })
    }
}

/// Why a thread action on Octavo failed.
#[derive(Debug, Error)]
pub enum ThreadActionError {
    #[error("Atuin Hub is off; `atuin setup --only --hub` turns it on")]
    Off,
    #[error("not logged in to Atuin Hub; run `atuin login`")]
    LoggedOut,
    #[error("Atuin Hub takes no thread of this harness or session id")]
    NotShareable,
    #[error("Atuin Hub doesn't store this thread yet; it goes up with its next message")]
    NotStored,
    #[error(
        "Atuin Hub didn't list this thread among your most recently active; it goes up, or moves \
         up, with its next message"
    )]
    NotAmongRecent,
    #[error(
        "this thread was captured on another machine, which holds its shares; run `atuin hub \
         share` there"
    )]
    CapturedElsewhere,
    #[error("failed to log in to Atuin Hub")]
    Login(#[source] HubCallError),
    #[error(transparent)]
    Hub(#[from] HubCallError),
    #[error("Atuin Hub's address can't hold a share link")]
    LinkUrl(#[from] url::ParseError),
    #[error(
        "this thread is private: `atuin hub unshare` stopped its sharing, and a link would share \
         it again"
    )]
    Private,
    #[error("failed to read the Atuin Hub upload queue")]
    Queue(#[from] QueueError),
}

impl ThreadActionError {
    /// Join this error and its causes into one line for the client to show.
    pub(crate) fn causes(&self) -> String {
        let err: &(dyn StdError + 'static) = self;
        std::iter::successors(Some(err), |err| (*err).source())
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(": ")
    }
}

#[derive(Debug, Error)]
pub enum ThreadSharesError {
    #[error("Atuin Hub takes no thread of this harness or session id")]
    NotShareable,
    #[error(
        "this thread was captured on another machine, which holds its shares; run `atuin hub \
         unshare` there"
    )]
    CapturedElsewhere,
    #[error("failed to read the Atuin Hub upload queue")]
    Queue(#[from] QueueError),
}

/// Thread messages taken for Octavo before capture stores their records: they go once
/// [`Self::start`] says the records are stored, so no upload reads the queue ahead of them.
#[must_use = "unless started, queued messages wait for a later upload, and direct ones never go"]
#[derive(Debug)]
pub enum ThreadUpload {
    /// None go: Octavo is off here, nobody is logged in, or they couldn't be queued.
    Nothing,
    /// Queued under `user`, for the engine `wake` wakes to send.
    Queued {
        wake: Arc<Notify>,
        user: UserId,
    },
    /// Left out of the queue, as the account had Octavo off when last asked: uploaded once,
    /// directly, and queued only if that fails for a reason that may pass and the mark has cleared
    /// since.
    Direct(DirectThreadUpload),
}

impl ThreadUpload {
    /// The user the messages go to Octavo as, if they go.
    #[must_use]
    pub fn user(&self) -> Option<&UserId> {
        match self {
            Self::Nothing => None,
            Self::Queued { user, .. } => Some(user),
            Self::Direct(upload) => Some(upload.user()),
        }
    }

    /// Starts the upload, the messages' records stored.
    pub fn start(self) {
        match self {
            Self::Nothing => {}
            Self::Queued { wake, .. } => wake.notify_one(),
            Self::Direct(upload) => upload.start(),
        }
    }
}

/// Whether the user's own config file turns Atuin Hub on. One that can't be run leaves it
/// off.
async fn threads_on(configs: &Configs) -> bool {
    configs.threads_on().await.unwrap_or_else(|err| {
        tracing::warn!(?err, "leaving Atuin Hub off, as your own config file can't be run");
        false
    })
}

#[derive(Debug)]
pub struct Octavo {
    client: AnyOctavoClient,
}

impl Octavo {
    /// Opens the client for `settings`, reading queued thread messages from `records`, and where
    /// threads started from `sidecar`, if there is one.
    pub async fn open(
        settings: &Settings,
        handle: DaemonHandle,
        records: AiSessionStore,
        sidecar: Option<AiSessionDatabase>,
    ) -> Self {
        // Once Atuin for Teams has been on here, the client opens with it off too. Its passes
        // read the config files again, so turning threads back on, or fixing a file that fails to
        // run, takes effect without a daemon restart.
        let configs = Configs::new(dirs::try_atuin_home().ok().map(PathBuf::from));
        if !settings.octavo_queue_path().exists() && !threads_on(&configs).await {
            return Self::nop();
        }

        let client = client::ActiveOctavoClient::open(settings, handle, records, sidecar);
        match client.await {
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

    #[cfg(test)]
    pub fn recording(client: RecordingOctavoClient) -> Self {
        Self {
            client: client.into(),
        }
    }

    /// Queues the upload of `messages`, captured on this host, which goes once their records are
    /// stored and the returned [`ThreadUpload`] started.
    pub async fn queue_thread_messages(&self, messages: &[Message]) -> ThreadUpload {
        self.client.queue_thread_messages(messages).await
    }

    /// Hears that a capture as it happens, not an import, stored a row of `session`, whose thread
    /// `sidecar` holds and `user` uploads: its config files may share it. Never waits.
    pub fn consider_sharing(
        &self,
        user: UserId,
        session: HarnessSession,
        sidecar: &AiSessionDatabase,
    ) {
        self.client.consider_sharing(user, session, sidecar);
    }

    pub async fn thread_shares(
        &self,
        session: HarnessSession,
    ) -> Result<ThreadShares, ThreadSharesError> {
        self.client.thread_shares(session).await
    }

    /// Make `session`'s thread private, queueing its closes and the revocation of its links, and
    /// return its sharing state afterwards.
    pub async fn make_private(
        &self,
        session: HarnessSession,
    ) -> Result<ThreadShares, ThreadSharesError> {
        self.client.make_private(session).await
    }

    /// Get a link on the hub's web app that opens `session`'s thread to anyone holding it.
    pub async fn share_link(&self, session: HarnessSession) -> Result<Url, ThreadActionError> {
        self.client.share_link(session).await
    }

    /// Add the labels named `names` to `session`'s thread and return the names of all its labels.
    pub async fn add_labels(
        &self,
        session: HarnessSession,
        names: &[String],
    ) -> Result<Vec<String>, ThreadActionError> {
        self.client.add_labels(session, names).await
    }

    /// Hears that an agent started `session` in `dir`, before atuin captured a message of it.
    pub fn thread_started(&self, session: HarnessSession, dir: PathBuf) {
        self.client.thread_started(session, dir);
    }

    /// Who the thread an agent started as `id` will be shared with, once its first message is
    /// captured, or `None` when no agent started one by that id lately.
    pub async fn new_thread_shares(&self, id: &str) -> Option<ThreadShares> {
        self.client.new_thread_shares(id).await
    }
}
