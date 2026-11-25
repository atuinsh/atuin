//! The loop that runs the helper, and feeds its records to the store and the index.
//!
//! - [`task`]: the task that starts helpers, and waits between helpers for as long as the way
//!   that each helper ended requires.
//! - [`helper`]: one run of the helper. It speaks the link protocol, which [`link`] tracks,
//!   and routes the helper's records through [`records`].
//! - [`process`]: starts the helper's process.
//! - [`helper_file`]: notices a replacement of the helper's file, which can fix a failed helper.
//! - [`opened`]: the store and the index, and the tasks that maintain them.
//! - [`settings`]: what the supervisor takes from the settings.

mod helper;
mod helper_file;
mod link;
mod opened;
mod process;
mod records;
mod settings;
mod task;

#[cfg(test)]
mod tests;

use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::time::Duration;

use atuin_client::settings::Settings;
use atuin_domain::record::HostId;
use futures::FutureExt as _;
use opened::Storage;
pub use opened::{Opened, StorageDeleteError};
use parking_lot::Mutex;
#[cfg(test)]
pub use process::HelperProcess;
pub use process::{HelperCommand, Spawn};
use settings::HelperSettings;
use task::{Command, DeleteRequest, Task};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tokio::time::timeout;
use tracing::{error, warn};

use super::session_watch::SessionWatch;
use super::stop_file::{Requested, StopFile, StopFileError};
use super::store::{Deleted, Selection};
use super::{CaptureState, Published, StoreNotOpen};

/// How long the shutdown waits for the task to stop before it aborts the task.
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, thiserror::Error)]
pub enum DeleteError {
    #[error(transparent)]
    Storage(#[from] StorageDeleteError),
    #[error(transparent)]
    NotOpen(#[from] StoreNotOpen),
    /// The supervisor ended before the deletion ran, because the daemon is shutting down or
    /// the supervisor panicked.
    #[error("{SUPERVISOR_ENDED}")]
    SupervisorEnded,
}

/// A `start` or `stop` that failed.
#[derive(Debug, thiserror::Error)]
pub enum RequestError {
    /// The stop file could not be written. A stop took effect in this daemon, but a restart of
    /// the daemon may not keep it. A start didn't take effect.
    #[error(transparent)]
    StopFile(#[from] StopFileError),
    /// The supervisor ended before the request ran, because the daemon is shutting down or
    /// the supervisor panicked.
    #[error("{SUPERVISOR_ENDED}")]
    SupervisorEnded,
}

/// The message of a request that the supervisor can't carry out, because the supervisor ended.
const SUPERVISOR_ENDED: &str = "the desktop capture supervisor is no longer running, because the \
                                daemon is shutting down or the supervisor failed";

/// The handle of the task that runs the helper when the settings enable capture.
pub struct Supervisor {
    storage: Arc<Storage>,
    /// Keeps a stop when the task cannot carry it out.
    stop_file: StopFile,
    commands: mpsc::UnboundedSender<Command>,
    published: watch::Receiver<Published>,
    /// Publishes a stop that the task cannot carry out, because the task has ended.
    publisher: watch::Sender<Published>,
    task: Mutex<Option<JoinHandle<()>>>,
}

impl Supervisor {
    /// Reads the stop file, and starts the task, which opens the store if the desktop capture
    /// directory holds a store or capture is enabled. The task keeps the stop requests in
    /// `stop_file`.
    ///
    /// A store that fails to open shows as `Failed`, and the task tries again with a backoff.
    pub async fn start(
        settings: &Settings,
        host: HostId,
        spawner: Arc<dyn Spawn>,
        sessions: Box<dyn SessionWatch>,
        stop_file: StopFile,
    ) -> Self {
        let storage = Arc::new(Storage::new(settings, host));
        let (commands, receiver) = mpsc::unbounded_channel();
        let requested = stop_file.requested().await;
        let task = Task::new(
            spawner,
            sessions,
            Arc::clone(&storage),
            HelperSettings::from_settings(settings),
            stop_file.clone(),
            requested,
            receiver,
        );
        let published = task.subscribe();
        let publisher = task.publisher();
        Self {
            storage,
            stop_file,
            commands,
            published,
            publisher: publisher.clone(),
            task: Mutex::new(Some(tokio::spawn(supervise(task, publisher)))),
        }
    }

    /// What the task publishes: its state, and whether the user stopped capture.
    #[must_use]
    pub fn published(&self) -> watch::Receiver<Published> {
        self.published.clone()
    }

    #[must_use]
    pub fn opened(&self) -> Option<&Opened> {
        self.storage.opened()
    }

    /// The store and the index, once they are open. If the store may open soon, the call waits
    /// up to `patience` for the store to open.
    pub async fn opened_within(&self, patience: Duration) -> Option<&Opened> {
        self.storage.opened_within(patience).await
    }

    /// Counts the attempts to open the store that have ended.
    #[cfg(test)]
    pub fn open_attempts(&self) -> watch::Receiver<u32> {
        self.storage.open_attempts()
    }

    /// Carries out `requested`, and waits until the state and the stop file reflect it.
    ///
    /// The request runs to its end even when the caller stops waiting. A stop that the task
    /// cannot carry out, because the task has ended, still writes the stop file, so that the
    /// next daemon keeps capture stopped.
    pub async fn request(&self, requested: Requested) -> Result<(), RequestError> {
        let (done, applied) = oneshot::channel();
        let answer = match self.commands.send(Command::Request { requested, done }) {
            Ok(()) => applied.await.ok(),
            Err(_) => None,
        };
        if let Some(answer) = answer {
            return Ok(answer?);
        }
        if requested == Requested::Stopped {
            self.stop_file.create().await?;
            self.publisher.send_if_modified(|published| {
                let changed = published.requested != Requested::Stopped;
                published.requested = Requested::Stopped;
                changed
            });
        }
        Err(RequestError::SupervisorEnded)
    }

    /// Deletes the segments that `selection` selects, between two runs of the helper.
    pub async fn delete(&self, selection: Selection) -> Result<Deleted, DeleteError> {
        let (done, deleted) = oneshot::channel();
        self.commands
            .send(Command::Delete(DeleteRequest { selection, done }))
            .map_err(|_| DeleteError::SupervisorEnded)?;
        deleted.await.map_err(|_| DeleteError::SupervisorEnded)?
    }

    pub fn apply_settings(&self, settings: &Settings) {
        let _ = self.commands.send(Command::Settings(HelperSettings::from_settings(settings)));
    }

    /// Starts a helper soon if capture is waiting for a desktop session, because a session has
    /// started.
    pub fn session_started(&self) {
        let _ = self.commands.send(Command::SessionStarted);
    }

    /// Stops the helper, the reconciler, and the GC, and then closes the store.
    pub async fn shutdown(&self) {
        let _ = self.commands.send(Command::Shutdown);
        let task = self.task.lock().take();
        if let Some(mut task) = task
            && timeout(SHUTDOWN_TIMEOUT, &mut task).await.is_err()
        {
            warn!(
                "the desktop capture supervisor did not stop within {} s, so it is aborted",
                SHUTDOWN_TIMEOUT.as_secs()
            );
            task.abort();
            let _ = task.await;
        }
        if let Some(opened) = self.storage.opened() {
            opened.close().await;
        }
    }
}

/// Runs `task`, and shows a panic of the task as `Failed` in `published`.
async fn supervise(task: Task, published: watch::Sender<Published>) {
    let Err(panic) = AssertUnwindSafe(task.run()).catch_unwind().await else {
        return;
    };
    let message = atuin_common::panic::message(&*panic);
    error!(panic = message, "the desktop capture supervisor panicked");
    published.send_modify(|published| {
        published.state = CaptureState::Failed {
            error: format!("the desktop capture supervisor panicked: {message}"),
        };
    });
}
