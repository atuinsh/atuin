mod client;

use std::path::PathBuf;
use std::sync::Arc;

use atuin_client::ai_session::{AiSessionDatabase, AiSessionStore, HarnessSession, Message};
use atuin_client::settings::Settings;
use atuin_common::dirs;
pub use atuin_octavo::hub::UserId;
use atuin_octavo::shares::Configs;
#[cfg(test)]
pub use client::RecordingOctavoClient;
use client::{AnyOctavoClient, DirectThreadUpload, NopOctavoClient, OctavoClient};
use tokio::sync::Notify;

use crate::daemon::DaemonHandle;

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
}
