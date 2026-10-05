use std::num::NonZeroU32;
use std::ops::ControlFlow;
use std::sync::Arc;
use std::time::Duration;

use atuin_client::history::HistoryId;
use atuin_common::futures::Backoff;
use atuin_octavo::hub::{HubCallError, HubClient, HubLogin};
use atuin_octavo::queue::{PendingHistoryUpload, UploadQueue};
use futures::StreamExt;
use tokio::sync::Notify;
use tokio::task::JoinHandle;

use crate::daemon::DaemonHandle;

const POLL_INTERVAL: Duration = Duration::from_secs(5 * 60);
const BACKOFF: Backoff = Backoff::Exponential {
    initial: Duration::from_secs(5),
    max: Duration::from_secs(30 * 60),
    factor: NonZeroU32::new(2).unwrap(),
};

#[derive(Debug)]
pub struct UploadEngine {
    task: JoinHandle<()>,
}

impl UploadEngine {
    pub fn spawn(
        queue: UploadQueue,
        hub: HubClient,
        handle: DaemonHandle,
        wake: Arc<Notify>,
    ) -> Self {
        let inner = EngineInner {
            queue,
            hub,
            handle,
            wake,
        };

        Self {
            task: tokio::spawn(inner.run()),
        }
    }
}

impl Drop for UploadEngine {
    fn drop(&mut self) {
        self.task.abort();
    }
}

struct RetryLater;

struct EngineInner {
    queue: UploadQueue,
    hub: HubClient,
    handle: DaemonHandle,
    wake: Arc<Notify>,
}

impl EngineInner {
    async fn run(self) {
        loop {
            BACKOFF
                .retry_forever(|| async {
                    self.send_pending().await.map_or(ControlFlow::Continue(()), ControlFlow::Break)
                })
                .await;

            tokio::select! {
                () = self.wake.notified() => {}
                () = tokio::time::sleep(POLL_INTERVAL) => {}
            }
        }
    }

    /// Sends the logged-in user's queued deletions, then, while Octavo is on, their queued
    /// uploads. What other users queued waits for them to log in again.
    async fn send_pending(&self) -> Result<(), RetryLater> {
        let settings = self.handle.settings().await.clone();
        let login = match self.hub.login(&settings).await {
            Ok(login) => login,
            Err(HubCallError::NotLoggedIn) => return Ok(()),
            Err(err) => {
                tracing::warn!(?err, "failed to log in to octavo; retrying later");
                return Err(RetryLater);
            }
        };

        self.delete_pending(&login).await?;
        if !settings.octavo.enabled {
            return Ok(());
        }

        let mut pending = self.queue.pending_history(login.user_id()).items();
        while let Some(entry) = pending.next().await {
            let entry = entry.map_err(|err| {
                tracing::warn!(?err, "failed to read the octavo upload queue; retrying later");
                RetryLater
            })?;
            self.upload(&login, entry).await?;
        }

        Ok(())
    }

    async fn upload(
        &self,
        login: &HubLogin,
        PendingHistoryUpload {
            history_id: id,
            record_id,
        }: PendingHistoryUpload,
    ) -> Result<(), RetryLater> {
        let history = self.handle.history_db().load(id).await.map_err(|err| {
            tracing::warn!(?err, %id, "failed to load a queued history entry; retrying later");
            RetryLater
        })?;
        let Some(history) = history.filter(|history| history.deleted_at.is_none()) else {
            return self.dequeue(login, id).await;
        };

        match login.upload(history, record_id).await {
            Ok(()) => {}
            Err(err) if err.is_permanent() => {
                tracing::warn!(
                    ?err,
                    %id,
                    "octavo will never accept this history entry; dropping it"
                );
            }
            Err(err) => {
                tracing::warn!(
                    ?err,
                    %id,
                    "failed to upload a history entry to octavo; retrying later"
                );
                return Err(RetryLater);
            }
        }

        self.dequeue(login, id).await
    }

    async fn delete_pending(&self, login: &HubLogin) -> Result<(), RetryLater> {
        loop {
            let ids = self.queue.pending_deletions(login.user_id()).await.map_err(|err| {
                tracing::warn!(?err, "failed to read the octavo deletion queue; retrying later");
                RetryLater
            })?;
            if ids.is_empty() {
                return Ok(());
            }

            for id in ids {
                self.delete(login, id).await?;
            }
        }
    }

    async fn delete(&self, login: &HubLogin, id: HistoryId) -> Result<(), RetryLater> {
        match login.delete(id).await {
            Ok(()) => {}
            Err(err) if err.is_permanent() => {
                tracing::warn!(
                    ?err,
                    %id,
                    "octavo refused to delete this history entry; dropping the deletion"
                );
            }
            Err(err) => {
                tracing::warn!(
                    ?err,
                    %id,
                    "failed to delete a history entry from octavo; retrying later"
                );
                return Err(RetryLater);
            }
        }

        self.queue.remove_deletion(login.user_id(), id).await.map_err(|err| {
            tracing::warn!(
                ?err,
                %id,
                "failed to remove a deletion from the octavo queue; retrying later"
            );
            RetryLater
        })
    }

    async fn dequeue(&self, login: &HubLogin, id: HistoryId) -> Result<(), RetryLater> {
        self.queue.remove_history(login.user_id(), id).await.map_err(|err| {
            tracing::warn!(
                ?err,
                %id,
                "failed to remove a history entry from the octavo upload queue; retrying later"
            );
            RetryLater
        })
    }
}
