use std::num::NonZeroU32;
use std::ops::ControlFlow;
use std::sync::Arc;
use std::time::Duration;

use atuin_client::ai_session::{AiSessionStore, Loaded, Message};
use atuin_client::history::HistoryId;
use atuin_client::record::decode::map_parallel;
use atuin_client::settings::Settings;
use atuin_common::futures::Backoff;
use atuin_domain::record::{HostId, RecordId, RecordIdx, RecordSeriesKey, RecordTag};
use atuin_octavo::hub::{HubCallError, HubClient, HubLogin};
use atuin_octavo::pb;
use atuin_octavo::queue::{
    PendingHistoryUpload, PendingOutputUpload, PendingThreadMessageUpload, ThreadBackfill,
    UploadQueue,
};
use futures::StreamExt;
use tokio::sync::Notify;
use tokio::task::JoinHandle;
use tonic::Code;
use uuid::Uuid;

use crate::daemon::DaemonHandle;
use crate::output_capture::OutputStore;

const POLL_INTERVAL: Duration = Duration::from_secs(5 * 60);
const BACKOFF: Backoff = Backoff::Exponential {
    initial: Duration::from_secs(5),
    max: Duration::from_secs(30 * 60),
    factor: NonZeroU32::new(2).unwrap(),
};

/// How long a queued output, or a queued thread message, waits to reach Octavo before it's
/// dropped.
///
/// An output waits while its entry is unfinished on this machine or missing from Octavo. The
/// entry is then in flight on the direct path, which gives up after `network_timeout` (30s by
/// default), or never coming: refused, stored under another login, or never uploaded. An output
/// that cannot be read from local storage waits out the same hour. A thread message waits while
/// its record isn't stored: capture queues it just before the push, which may fail.
const ORPHAN_AFTER: Duration = Duration::from_secs(60 * 60);

/// How long a pass that something woke the engine for waits before it uploads thread messages.
///
/// Capture stores a Claude Code reply as a row per block, each waking the engine, so the rows of
/// one reply go up in one request rather than one each. A row not stored by then goes on the
/// next pass.
const COALESCE_WINDOW: Duration = Duration::from_secs(2);

/// The most records one pass reads of a backfill: as many as one `InsertThreadMessages` takes.
const BACKFILL_PAGE: u64 = pb::ThreadBatch::MAX_MESSAGES.get() as u64;

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
        records: AiSessionStore,
        wake: Arc<Notify>,
    ) -> Self {
        let inner = EngineInner {
            queue,
            hub,
            handle,
            outputs,
            records,
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

/// When the run loop starts its next pass.
enum NextPass {
    /// Once something wakes the engine, or at the poll interval.
    Waiting,
    /// At once: a backfill has more to go.
    Now,
}

struct EngineInner {
    queue: UploadQueue,
    hub: HubClient,
    handle: DaemonHandle,
    outputs: Arc<OutputStore>,
    /// Where queued thread messages are read from.
    records: AiSessionStore,
    wake: Arc<Notify>,
}

impl EngineInner {
    async fn run(self) {
        // Held messages wait for a key or a build this daemon didn't start with, which only a
        // restart loads.
        if let Err(err) = self.queue.release_held_thread_messages().await {
            tracing::warn!(?err, "failed to release held thread messages; they wait for a restart");
        }

        let mut woken = false;
        loop {
            // Only a pass's first attempt waits to coalesce; a retry has waited out its backoff.
            let mut coalesce = woken;
            let next = BACKOFF
                .retry_forever(|| {
                    let engine = &self;
                    let coalesce = std::mem::take(&mut coalesce);
                    async move {
                        engine
                            .send_pending(coalesce)
                            .await
                            .map_or(ControlFlow::Continue(()), ControlFlow::Break)
                    }
                })
                .await;

            woken = match next {
                NextPass::Now => false,
                NextPass::Waiting => tokio::select! {
                    () = self.wake.notified() => true,
                    () = tokio::time::sleep(POLL_INTERVAL) => false,
                },
            };
        }
    }

    /// Sends the logged-in user's queued deletions, then, while Octavo is on, their queued
    /// uploads, outputs (while output uploads are on) and thread messages, the last after
    /// [`COALESCE_WINDOW`] with `coalesce`. What other users queued waits for them to log in
    /// again.
    async fn send_pending(&self, coalesce: bool) -> Result<NextPass, RetryLater> {
        let settings = self.handle.settings().await.clone();
        let login = match self.hub.login(&settings).await {
            Ok(login) => login,
            Err(HubCallError::NotLoggedIn) => return Ok(NextPass::Waiting),
            Err(err) => {
                tracing::warn!(?err, "failed to log in to octavo; retrying later");
                return Err(RetryLater);
            }
        };

        self.delete_pending(&login).await?;
        if !settings.octavo.enabled {
            return Ok(NextPass::Waiting);
        }

        let mut pending = self.queue.pending_history(login.user_id()).items();
        while let Some(entry) = pending.next().await {
            let entry = entry.map_err(|err| {
                tracing::warn!(?err, "failed to read the octavo upload queue; retrying later");
                RetryLater
            })?;
            self.upload(&login, entry).await?;
        }

        self.send_outputs(&login, &settings).await?;
        if coalesce {
            tokio::time::sleep(COALESCE_WINDOW).await;
        }
        self.send_threads(&login).await
    }

    async fn send_outputs(&self, login: &HubLogin, settings: &Settings) -> Result<(), RetryLater> {
        if !settings.octavo.upload_output {
            return Ok(());
        }

        let mut pending = self.queue.pending_outputs(login.user_id()).items();
        while let Some(output) = pending.next().await {
            let PendingOutputUpload { history_id } = output.map_err(|err| {
                tracing::warn!(?err, "failed to read the octavo output queue; retrying later");
                RetryLater
            })?;
            self.upload_output(login, history_id).await?;
        }

        Ok(())
    }

    /// Sends the queued thread messages of `login`'s user, then a page of their first backfill
    /// not done.
    async fn send_threads(&self, login: &HubLogin) -> Result<NextPass, RetryLater> {
        self.fix_backfill_bounds(login).await?;

        let mut pending = self.queue.pending_thread_messages(login.user_id());
        while let Some(page) = pending.next().await {
            let ids = page
                .into_iter()
                .map(|upload| upload.map(|PendingThreadMessageUpload { record_id }| record_id))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|err| {
                    tracing::warn!(?err, "failed to read the octavo thread queue; retrying later");
                    RetryLater
                })?;
            self.send_thread_page(login, &ids).await?;
        }

        self.backfill(login).await
    }

    /// Uploads the queued thread messages `ids`, and settles each: dequeued once uploaded or
    /// refused, held while this build can't read its record, and expired while its record isn't
    /// stored.
    async fn send_thread_page(&self, login: &HubLogin, ids: &[RecordId]) -> Result<(), RetryLater> {
        let user = login.user_id();
        let loaded = self.records.load(ids).await.map_err(|err| {
            tracing::warn!(?err, "failed to load queued thread messages; retrying later");
            RetryLater
        })?;

        let mut messages = Vec::new();
        let mut settled = Vec::new();
        let mut held = Vec::new();
        let mut missing = Vec::new();
        for (id, loaded) in loaded {
            match loaded {
                Loaded::Message(message) => {
                    settled.push(id);
                    messages.push(*message);
                }
                Loaded::Skipped => settled.push(id),
                Loaded::Held => held.push(id),
                Loaded::Missing => missing.push(id),
            }
        }

        self.queue.hold_thread_messages(user, held).await.map_err(|err| {
            tracing::warn!(?err, "failed to hold unreadable thread messages; retrying later");
            RetryLater
        })?;
        match self.queue.expire_thread_messages(user, missing, ORPHAN_AFTER).await {
            Ok(0) => {}
            Ok(expired) => tracing::warn!(
                expired,
                "dropping thread messages that waited an hour without being stored"
            ),
            Err(err) => {
                tracing::warn!(?err, "failed to expire queued thread messages; retrying later");
                return Err(RetryLater);
            }
        }

        upload_thread_messages(login, converted(messages).await).await?;
        self.queue.remove_thread_messages(user, settled).await.map_err(|err| {
            tracing::warn!(
                ?err,
                "failed to remove thread messages from the octavo upload queue; retrying later"
            );
            RetryLater
        })
    }

    /// Fixes the bound of each of `login`'s backfills that has none just past its series' tail.
    ///
    /// This runs with uploads on, so every record the capture hook let by while they were off is
    /// at or below the tail. The exception: an append whose hook read the setting just before
    /// the daemon applied it, and whose push lands after this; running the backfill again
    /// recovers it.
    async fn fix_backfill_bounds(&self, login: &HubLogin) -> Result<(), RetryLater> {
        let user = login.user_id();
        let backfills = self.queue.thread_backfills(user).await.map_err(|err| {
            tracing::warn!(?err, "failed to read the octavo thread backfills; retrying later");
            RetryLater
        })?;

        let mut tails = Vec::new();
        for ThreadBackfill { host_id, .. } in
            backfills.into_iter().filter(|backfill| backfill.until_idx.is_none())
        {
            let series = RecordSeriesKey::new(host_id, RecordTag::AiSession);
            let tail = self.handle.store().last(&series).await.map_err(|err| {
                tracing::warn!(?err, "failed to read a thread series' tail; retrying later");
                RetryLater
            })?;
            // A series gone since the backfill was asked for stays unfixed, and unwalked, until
            // it is back.
            if let Some(tail) = tail {
                tails.push((host_id, tail.idx));
            }
        }

        self.queue.fix_thread_backfills(user, tails).await.map_err(|err| {
            tracing::warn!(?err, "failed to fix the octavo thread backfills; retrying later");
            RetryLater
        })
    }

    /// Uploads a page of `login`'s first backfill not done, and moves it on past the page.
    async fn backfill(&self, login: &HubLogin) -> Result<NextPass, RetryLater> {
        let user = login.user_id();
        let backfills = self.queue.thread_backfills(user).await.map_err(|err| {
            tracing::warn!(?err, "failed to read the octavo thread backfills; retrying later");
            RetryLater
        })?;

        for ThreadBackfill {
            host_id,
            next_idx,
            until_idx,
        } in backfills
        {
            let Some(until_idx) = until_idx.filter(|&until| next_idx < until) else {
                continue;
            };
            let series = RecordSeriesKey::new(host_id, RecordTag::AiSession);
            let read = self.records.page(&series, next_idx..until_idx, BACKFILL_PAGE);
            let page = read.await.map_err(|err| {
                tracing::warn!(?err, "failed to read a thread backfill; retrying later");
                RetryLater
            })?;

            // Nothing is stored in what is left of it.
            let Some(&(last, _, _)) = page.last() else {
                self.advance_backfill(login, host_id, until_idx).await?;
                continue;
            };
            self.send_backfill_page(login, page).await?;
            self.advance_backfill(login, host_id, last + 1).await?;
            return Ok(NextPass::Now);
        }

        Ok(NextPass::Waiting)
    }

    /// Uploads `page` of a backfill, and queues its records this build can't read, held, for a
    /// restarted daemon to upload.
    async fn send_backfill_page(
        &self,
        login: &HubLogin,
        page: Vec<(RecordIdx, RecordId, Loaded)>,
    ) -> Result<(), RetryLater> {
        let user = login.user_id();
        let mut messages = Vec::new();
        let mut held = Vec::new();
        for (_, id, loaded) in page {
            match loaded {
                Loaded::Message(message) => messages.push(*message),
                Loaded::Held => held.push(id),
                Loaded::Missing | Loaded::Skipped => {}
            }
        }

        let queued = async {
            self.queue.push_thread_messages(user, held.iter().copied()).await?;
            self.queue.hold_thread_messages(user, held).await
        };
        queued.await.map_err(|err| {
            tracing::warn!(?err, "failed to queue unreadable thread messages; retrying later");
            RetryLater
        })?;

        upload_thread_messages(login, converted(messages).await).await
    }

    async fn advance_backfill(
        &self,
        login: &HubLogin,
        host: HostId,
        next: RecordIdx,
    ) -> Result<(), RetryLater> {
        let advanced = self.queue.advance_thread_backfill(login.user_id(), host, next);
        advanced.await.map_err(|err| {
            tracing::warn!(?err, "failed to move an octavo thread backfill on; retrying later");
            RetryLater
        })
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

/// `messages` as Octavo takes them, less those it never will, which are dropped with a warning.
async fn converted(messages: Vec<Message>) -> Vec<pb::CapturedThreadMessage> {
    let converted = map_parallel(messages, |message| {
        let id = message.id;
        (id, pb::CapturedThreadMessage::try_from(message))
    });

    converted
        .await
        .into_iter()
        .filter_map(|(id, converted)| {
            converted
                .inspect_err(|err| {
                    tracing::warn!(
                        ?err,
                        id = %id.0,
                        "octavo will never accept this thread message; dropping it"
                    );
                })
                .ok()
        })
        .collect()
}

/// Uploads `messages`, dropping with a warning each one Octavo refuses.
///
/// A request Octavo refuses, or that runs out of time, goes again in halves, so one too large for
/// the server or the uplink shrinks instead of repeating; a lone message that runs out of time is
/// retried later.
async fn upload_thread_messages(
    login: &HubLogin,
    messages: Vec<pb::CapturedThreadMessage>,
) -> Result<(), RetryLater> {
    for batch in pb::ThreadBatch::batches(messages) {
        let mut requests = vec![batch];
        while let Some(request) = requests.pop() {
            let Err(err) = login.upload_thread_messages(request.clone()).await else {
                continue;
            };
            match (UploadFailure::of(&err), request.halves()) {
                (UploadFailure::Transient, _) | (UploadFailure::TimedOut, Err(_)) => {
                    tracing::warn!(
                        ?err,
                        "failed to upload thread messages to octavo; retrying later"
                    );
                    return Err(RetryLater);
                }
                (UploadFailure::Refused, Err(lone)) => tracing::warn!(
                    ?err,
                    id = ?lone.messages().first().and_then(record_id),
                    "octavo will never accept this thread message; dropping it"
                ),
                (UploadFailure::Refused | UploadFailure::TimedOut, Ok((front, back))) => {
                    // The front half is popped first, which keeps the messages in order.
                    requests.extend([back, front]);
                }
            }
        }
    }

    Ok(())
}

/// Why an `InsertThreadMessages` request failed, as [`upload_thread_messages`] acts on it.
enum UploadFailure {
    /// Octavo will never store one of its messages.
    Refused,
    /// It didn't go up inside `network_timeout`, which a smaller request may.
    TimedOut,
    /// It may go up as it is on a later try.
    Transient,
}

impl UploadFailure {
    fn of(err: &HubCallError) -> Self {
        let HubCallError::Hub(status) = err else {
            return if err.is_permanent() {
                Self::Refused
            } else {
                Self::Transient
            };
        };
        match status.code() {
            // The server answers a request body over its size limit with RESOURCE_EXHAUSTED, and
            // raises it for nothing else.
            Code::InvalidArgument | Code::ResourceExhausted => Self::Refused,
            // tonic fails a call that outlives its deadline as CANCELLED.
            Code::Cancelled | Code::DeadlineExceeded => Self::TimedOut,
            Code::Ok
            | Code::Unknown
            | Code::NotFound
            | Code::AlreadyExists
            | Code::PermissionDenied
            | Code::FailedPrecondition
            | Code::Aborted
            | Code::OutOfRange
            | Code::Unimplemented
            | Code::Internal
            | Code::Unavailable
            | Code::DataLoss
            | Code::Unauthenticated => Self::Transient,
        }
    }
}

/// The id of the record `message` was converted from.
fn record_id(message: &pb::CapturedThreadMessage) -> Option<Uuid> {
    let id = message.message.as_ref()?.id.as_ref()?;
    Uuid::from_slice(&id.value).ok()
}
