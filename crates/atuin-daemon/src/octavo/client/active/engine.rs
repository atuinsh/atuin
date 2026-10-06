use std::num::NonZeroU32;
use std::ops::ControlFlow;
use std::sync::Arc;
use std::time::Duration;

use atuin_client::history::HistoryId;
use atuin_common::futures::Backoff;
use atuin_octavo::hub::{HubCallError, HubClient, HubLogin};
use atuin_octavo::queue::{PendingHistoryUpload, PendingOutputUpload, UploadQueue};
use futures::StreamExt;
use tokio::sync::Notify;
use tokio::task::JoinHandle;

use crate::daemon::DaemonHandle;
use crate::output_capture::OutputStore;

const POLL_INTERVAL: Duration = Duration::from_secs(5 * 60);
const BACKOFF: Backoff = Backoff::Exponential {
    initial: Duration::from_secs(5),
    max: Duration::from_secs(30 * 60),
    factor: NonZeroU32::new(2).unwrap(),
};

/// How long a queued output waits to reach Octavo before it's dropped.
///
/// An output waits while its entry is unfinished on this machine or missing from Octavo. The
/// entry is then in flight on the direct path, which gives up after `network_timeout` (30s by
/// default), or never coming: refused, stored under another login, or never uploaded. An output
/// that cannot be read from local storage waits out the same hour.
const ORPHAN_AFTER: Duration = Duration::from_secs(60 * 60);

#[derive(Debug)]
pub struct UploadEngine {
    task: JoinHandle<()>,
}

impl UploadEngine {
    pub fn spawn(
        queue: UploadQueue,
        hub: HubClient,
        handle: DaemonHandle,
        outputs: Arc<OutputStore>,
        wake: Arc<Notify>,
    ) -> Self {
        let inner = EngineInner {
            queue,
            hub,
            handle,
            outputs,
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
    outputs: Arc<OutputStore>,
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
    /// uploads, then, while output uploads are on, their queued outputs. What other users queued
    /// waits for them to log in again.
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

        if !settings.octavo.upload_output {
            return Ok(());
        }

        let mut pending = self.queue.pending_outputs(login.user_id()).items();
        while let Some(output) = pending.next().await {
            let PendingOutputUpload { history_id } = output.map_err(|err| {
                tracing::warn!(?err, "failed to read the octavo output queue; retrying later");
                RetryLater
            })?;
            self.upload_output(&login, history_id).await?;
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
                if let Err(err) = self.queue.remove_output(login.user_id(), id).await {
                    tracing::warn!(
                        ?err,
                        %id,
                        "failed to remove the output of a refused history entry from the queue"
                    );
                }
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

    async fn upload_output(&self, login: &HubLogin, id: HistoryId) -> Result<(), RetryLater> {
        // Octavo can't have an entry `history end` hasn't stored here; its upload wakes the engine.
        if !matches!(self.handle.history_db().load(id).await, Ok(Some(_))) {
            return self.expire_output(login, id).await;
        }

        let capture = match self.outputs.get(id).await {
            Ok(capture) => capture,
            Err(err) => {
                tracing::warn!(?err, %id, "failed to read a queued output; skipping it");
                return self.expire_output(login, id).await;
            }
        };
        // Deleted or collected locally since it was queued.
        let Some(capture) = capture else {
            return self.dequeue_output(login, id).await;
        };

        match login.upload_output(id, capture).await {
            Ok(()) => {}
            Err(HubCallError::HistoryNotStored(_)) => {
                tracing::debug!(%id, "octavo has not stored this output's entry yet");
                return self.expire_output(login, id).await;
            }
            Err(err) if err.is_permanent() => {
                tracing::warn!(?err, %id, "octavo will never accept this output; dropping it");
            }
            Err(err) => {
                tracing::warn!(?err, %id, "failed to upload an output to octavo; retrying later");
                return Err(RetryLater);
            }
        }

        self.dequeue_output(login, id).await
    }

    /// Leaves the output of `id` queued to wait for its entry, unless it has waited
    /// [`ORPHAN_AFTER`].
    async fn expire_output(&self, login: &HubLogin, id: HistoryId) -> Result<(), RetryLater> {
        match self.queue.expire_output(login.user_id(), id, ORPHAN_AFTER).await {
            Ok(true) => {
                tracing::warn!(
                    %id,
                    "dropping an output that waited an hour without reaching octavo"
                );
                Ok(())
            }
            Ok(false) => Ok(()),
            Err(err) => {
                tracing::warn!(?err, %id, "failed to expire a queued output; retrying later");
                Err(RetryLater)
            }
        }
    }

    async fn dequeue_output(&self, login: &HubLogin, id: HistoryId) -> Result<(), RetryLater> {
        self.queue.remove_output(login.user_id(), id).await.map_err(|err| {
            tracing::warn!(
                ?err,
                %id,
                "failed to remove an output from the octavo upload queue; retrying later"
            );
            RetryLater
        })
    }
}
