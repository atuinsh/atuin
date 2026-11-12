use std::collections::{HashMap, HashSet};
use std::num::NonZeroU32;
use std::ops::ControlFlow;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use atuin_client::ai_session::{AiSessionStore, Loaded, Message};
use atuin_client::history::HistoryId;
use atuin_client::record::decode::map_parallel;
use atuin_client::settings::Settings;
use atuin_common::futures::{Backoff, Debounce};
use atuin_domain::record::{HostId, RecordId, RecordIdx, RecordSeriesKey, RecordTag};
use atuin_octavo::hub::{HubCallError, HubClient, HubLogin};
use atuin_octavo::pb;
use atuin_octavo::queue::{
    NotSent, PendingHistoryUpload, PendingOpen, PendingOutputUpload, PendingThreadMessageUpload,
    QueueError, ThreadBackfill, UploadQueue,
};
use atuin_octavo::shares::Configs;
use futures::StreamExt;
use tokio::sync::Notify;
use tokio::sync::broadcast::Receiver;
use tokio::sync::broadcast::error::RecvError;
use tokio::task::JoinHandle;
use tonic::Code;
use uuid::Uuid;

use super::gate::{Cleared, CloseHalt, Closes, Opening, Rows};
use super::{clear_off, forget_output, is_off, mark_off};
use crate::daemon::DaemonHandle;
use crate::events::DaemonEvent;
use crate::output_capture::OutputStore;

const POLL_INTERVAL: Duration = Duration::from_secs(5 * 60);
const BACKOFF: Backoff = Backoff::Exponential {
    initial: Duration::from_secs(5),
    max: Duration::from_secs(30 * 60),
    factor: NonZeroU32::new(2).unwrap(),
};

/// How long a queued output, a queued thread message or a queued thread share waits to reach Octavo
/// before it's dropped.
///
/// An output waits while its entry is unfinished on this machine or missing from Octavo. The
/// entry is then in flight on the direct path, which gives up after `network_timeout` (30s by
/// default), or never coming: refused, stored under another login, or never uploaded. An output
/// that cannot be read from local storage waits out the same hour. A thread message waits while
/// its record isn't stored: capture queues it just before the push, which may fail. A thread share
/// waits while its thread isn't stored, whose messages normally go ahead of it, so that thread's
/// were refused or dropped, or the user deleted it; its next message queues the share afresh.
const ORPHAN_AFTER: Duration = Duration::from_secs(60 * 60);

/// How a woken pass waits for the wakes to stop. Capture stores a Claude Code reply as a row per
/// block, each waking the engine, so the rows of one reply go up in one request, and rows
/// captured without a pause still go within a second.
const DEBOUNCE: Debounce = Debounce {
    quiet: Duration::from_millis(100),
    max: Duration::from_secs(1),
};

/// The most records one pass reads of a backfill: as many as one `InsertThreadMessages` takes.
const BACKFILL_PAGE: u64 = pb::ThreadBatch::MAX_ITEMS.get() as u64;

/// How long one pass can spend on history uploads, command output uploads or thread shares, each,
/// to prevent starving thread uploads.
pub(super) const PASS_BUDGET: Duration = Duration::from_secs(5);

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
        configs: Configs,
    ) -> Self {
        let events = handle.subscribe();
        let inner = EngineInner {
            queue,
            hub,
            handle,
            outputs,
            records,
            wake,
            logins: Notify::new(),
            configs,
        };

        Self {
            task: tokio::spawn(inner.run(events)),
        }
    }
}

impl Drop for UploadEngine {
    fn drop(&mut self) {
        self.task.abort();
    }
}

pub(super) struct RetryLater;

/// Why a pass ends before it has sent all it can.
enum Halt {
    /// Something failed for a reason that may pass: the pass goes again after a backoff.
    RetryLater,
    /// Octavo refused an upload as off for the account, or answered one without storing it while
    /// the account is marked off. What is left stays queued, for a later pass to probe.
    Off,
}

/// [`upload`] failed for a reason that may pass.
#[derive(Debug, Clone, Copy)]
pub(super) struct UploadRetryLater {
    /// Whether Octavo stored a request before the failure, which shows the account has it on.
    pub stored: bool,
}

impl From<UploadRetryLater> for RetryLater {
    fn from(_: UploadRetryLater) -> Self {
        Self
    }
}

impl From<UploadRetryLater> for Halt {
    fn from(_: UploadRetryLater) -> Self {
        Self::RetryLater
    }
}

impl From<RetryLater> for Halt {
    fn from(RetryLater: RetryLater) -> Self {
        Self::RetryLater
    }
}

/// When the run loop starts its next pass.
#[derive(Clone, Copy, Eq, PartialEq)]
enum NextPass {
    /// Once something wakes the engine, at a login, or at the poll interval.
    Waiting,
    /// At once: more backfill, queued history, or queued outputs exist.
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
    /// Notified at each `atuin login`, which sends a pass that failed, or one waiting, at once.
    logins: Notify,
    /// Who threads are shared with.
    configs: Configs,
}

impl EngineInner {
    async fn run(self, events: Receiver<DaemonEvent>) {
        tokio::join!(self.send_forever(), self.hear_events(events));
    }

    async fn send_forever(&self) {
        // Held messages wait for a key or a build this daemon didn't start with, which only a
        // restart loads.
        if let Err(err) = self.queue.release_held_thread_messages().await {
            tracing::warn!(?err, "failed to release held thread messages; they wait for a restart");
        }

        let mut woken = false;
        loop {
            // Only a woken pass waits for quiet; a retry waits out its backoff instead.
            if woken {
                DEBOUNCE.settle(|| self.wake.notified()).await;
            }
            // Only a login cuts the backoff short, as a new session may let the failed pass
            // through; what a wake brings waits for the next attempt. The pass after a login
            // probes a paused queue, as one the poll starts does. Atomic only so this future
            // stays `Send`: both closures run on this task.
            let retry_woken = AtomicBool::new(woken);
            let next = BACKOFF
                .retry_forever_or_sooner(
                    || async {
                        self.send_pending(retry_woken.load(Ordering::Relaxed))
                            .await
                            .map_or(ControlFlow::Continue(()), ControlFlow::Break)
                    },
                    || async {
                        self.logins.notified().await;
                        retry_woken.store(false, Ordering::Relaxed);
                    },
                )
                .await;

            woken = match next {
                NextPass::Now => false,
                NextPass::Waiting => tokio::select! {
                    () = self.wake.notified() => true,
                    () = self.logins.notified() => false,
                    () = tokio::time::sleep(POLL_INTERVAL) => false,
                },
            };
        }
    }

    /// Passes each login the daemon hears of on to [`Self::logins`], and wakes the engine at each
    /// settings reload.
    async fn hear_events(&self, mut events: Receiver<DaemonEvent>) {
        loop {
            match events.recv().await {
                // The events a lag dropped may have held one.
                Ok(DaemonEvent::LoggedIn) | Err(RecvError::Lagged(_)) => self.logins.notify_one(),
                // `config enable octavo` clears the off mark before it writes the config.
                Ok(DaemonEvent::SettingsReloaded) => self.wake.notify_one(),
                Ok(DaemonEvent::ShutdownRequested) => {}
                Err(RecvError::Closed) => return,
            }
        }
    }

    /// Sends the logged-in user's queued deletions, then, while Octavo is on here, their queued
    /// uploads, outputs (while output uploads are on) and thread messages. What other users queued
    /// waits for them to log in again.
    ///
    /// While the account is marked off, a woken pass sends none of the uploads, and any other is a
    /// probe: it goes on past the first upload Octavo answers only if the answer finds Octavo on.
    async fn send_pending(&self, woken: bool) -> Result<NextPass, RetryLater> {
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
        // Fixed even while the account is marked off: what capture lets by then goes up direct or
        // not at all, and a bound fixed once the mark clears would take it.
        self.fix_backfill_bounds(&login).await?;
        // A woken pass leaves a paused queue be. Any other probes it, and the poll starts one at most
        // `POLL_INTERVAL` after the last wake: a stale mark lasts that long, for one refused round
        // trip a poll while the account is off.
        if woken && is_off(&self.queue, login.user_id()).await {
            return Ok(NextPass::Waiting);
        }

        match self.send_uploads(&login, &settings).await {
            Ok(next) => Ok(next),
            Err(Halt::Off) => Ok(NextPass::Waiting),
            Err(Halt::RetryLater) => Err(RetryLater),
        }
    }

    /// Sends `login`'s queued thread messages and up to a page of backfill, then queued history
    /// entries, then (only after all history is done) command output, then thread shares, all up
    /// to the first [`Halt::Off`].
    ///
    /// History uploads, command output uploads and thread shares are each bounded by
    /// [`PASS_BUDGET`].
    async fn send_uploads(&self, login: &HubLogin, settings: &Settings) -> Result<NextPass, Halt> {
        let threads = self.send_threads(login).await?;
        let commands = match self.send_history(login).await? {
            NextPass::Waiting => self.send_outputs(login, settings).await?,
            n @ NextPass::Now => n,
        };
        // Last, as a share waits for its thread's messages and must hold up no upload. One that
        // may go later waits for the next pass rather than backing off: a wake can't cut a backoff
        // short, so the thread messages it brings would wait it out too.
        if matches!(self.send_thread_shares(login).await, Err(Halt::Off)) {
            return Err(Halt::Off);
        }

        Ok(if [threads, commands].into_iter().any(|n| n == NextPass::Now) {
            NextPass::Now
        } else {
            NextPass::Waiting
        })
    }

    /// Uploads pages of `login`'s queued history entries for up to [`PASS_BUDGET`], saying whether
    /// more may be left.
    async fn send_history(&self, login: &HubLogin) -> Result<NextPass, Halt> {
        let deadline = tokio::time::Instant::now() + PASS_BUDGET;
        let mut pending = self.queue.pending_history(login.user_id());
        while let Some(page) = pending.next().await {
            if tokio::time::Instant::now() >= deadline {
                return Ok(NextPass::Now);
            }
            let page = page.into_iter().collect::<Result<Vec<_>, _>>().map_err(|err| {
                tracing::warn!(?err, "failed to read the octavo upload queue; retrying later");
                RetryLater
            })?;
            self.send_history_page(login, page).await?;
        }

        Ok(NextPass::Waiting)
    }

    async fn send_history_page(
        &self,
        login: &HubLogin,
        page: Vec<PendingHistoryUpload>,
    ) -> Result<(), Halt> {
        let user = login.user_id();
        let records: HashMap<_, _> =
            page.iter().map(|upload| (upload.history_id, upload.record_id)).collect();
        let loaded = self.handle.history_db().load_active(records.keys().copied()).await;
        let loaded = loaded.map_err(|err| {
            tracing::warn!(?err, "failed to load queued history entries; retrying later");
            RetryLater
        })?;

        let mut refused = Vec::new();
        let mut entries = Vec::with_capacity(loaded.len());
        for history in loaded {
            let id = history.id;
            let record_id = records.get(&id).copied().flatten();
            match pb::History::with_record(history, record_id) {
                Ok(entry) => entries.push(entry),
                Err(err) => {
                    tracing::warn!(
                        ?err,
                        %id,
                        "octavo will never accept this history entry; dropping it"
                    );
                    refused.push(id);
                }
            }
        }

        let sent = upload(
            entries,
            "history entries",
            |batch| login.upload_history(batch),
            |entry, err| {
                let id = history_id(entry);
                tracing::warn!(
                    ?err,
                    ?id,
                    "octavo will never accept this history entry; dropping it"
                );
                refused.extend(id);
            },
        )
        .await;
        let sent = match sent {
            Ok(sent) => sent,
            Err(UploadRetryLater { stored }) => {
                if stored {
                    clear_off(&self.queue, user).await;
                }
                return Err(Halt::RetryLater);
            }
        };
        match sent {
            Uploaded::Stored => {
                clear_off(&self.queue, user).await;
            }
            Uploaded::NoneStored | Uploaded::Nothing => {}
            Uploaded::Off => {
                tracing::debug!(
                    "octavo is off for the account; leaving its history entries queued"
                );
                mark_off(&self.queue, user).await;
                return Err(Halt::Off);
            }
        }

        for id in refused {
            forget_output(&self.queue, user, id).await;
        }
        self.queue.remove_history(user, records.into_keys()).await.map_err(|err| {
            tracing::warn!(
                ?err,
                "failed to remove history entries from the octavo upload queue; retrying later"
            );
            RetryLater
        })?;

        if sent == Uploaded::NoneStored {
            return self.halt_if_off(login).await;
        }
        Ok(())
    }

    /// Uploads `login`'s queued outputs for up to [`PASS_BUDGET`], saying whether more may be left.
    async fn send_outputs(&self, login: &HubLogin, settings: &Settings) -> Result<NextPass, Halt> {
        if !settings.octavo.upload_output {
            return Ok(NextPass::Waiting);
        }

        let deadline = tokio::time::Instant::now() + PASS_BUDGET;
        let mut pending = self.queue.pending_outputs(login.user_id()).items();
        while let Some(output) = pending.next().await {
            if tokio::time::Instant::now() >= deadline {
                return Ok(NextPass::Now);
            }
            let PendingOutputUpload { history_id } = output.map_err(|err| {
                tracing::warn!(?err, "failed to read the octavo output queue; retrying later");
                RetryLater
            })?;
            self.upload_output(login, history_id).await?;
        }

        Ok(NextPass::Waiting)
    }

    /// Sends the queued thread messages of `login`'s user, then a page of their first backfill
    /// not done.
    async fn send_threads(&self, login: &HubLogin) -> Result<NextPass, Halt> {
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
    /// stored. Those to upload stay queued while Octavo refuses them as off for the account.
    async fn send_thread_page(&self, login: &HubLogin, ids: &[RecordId]) -> Result<(), Halt> {
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
        // Those that must wait stay queued for the next pass.
        let Cleared { ready, waiting } = self.clear(login, messages).await?;
        let waiting: HashSet<_> = waiting.into_iter().collect();
        settled.retain(|id| !waiting.contains(id));

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
                return Err(Halt::RetryLater);
            }
        }

        let sent = upload_thread_messages(login, converted(ready).await).await?;
        match sent {
            Uploaded::Stored => {
                clear_off(&self.queue, user).await;
            }
            Uploaded::NoneStored | Uploaded::Nothing => {}
            Uploaded::Off => {
                // Those stored before the refusal go again, which Octavo takes as a repeat.
                tracing::debug!(
                    "octavo is off for the account; leaving its thread messages queued"
                );
                mark_off(&self.queue, user).await;
                return Err(Halt::Off);
            }
        }
        self.queue.remove_thread_messages(user, settled).await.map_err(|err| {
            tracing::warn!(
                ?err,
                "failed to remove thread messages from the octavo upload queue; retrying later"
            );
            RetryLater
        })?;

        if sent == Uploaded::NoneStored {
            return self.halt_if_off(login).await;
        }
        Ok(())
    }

    /// Sends `login`'s queued thread shares for up to [`PASS_BUDGET`], up to the first that fails
    /// for a reason that may pass, as one the budget cuts short does.
    ///
    /// A share cut short goes again, which Octavo takes as a repeat even if it stored the first.
    async fn send_thread_shares(&self, login: &HubLogin) -> Result<(), Halt> {
        let opening = Opening::of(&self.queue, login).await?;
        let narrowed = opening.look_again(&self.configs).await?.narrow().await?;
        // First, so no share opens to an organization one of its thread's directories keeps out.
        let deadline = tokio::time::Instant::now() + PASS_BUDGET;
        let sent = Closes::of(&self.queue, login).send(deadline).await?;
        for open in narrowed.pending(sent).await? {
            let sent = tokio::time::timeout_at(deadline, self.send_thread_share(login, open)).await;
            sent.unwrap_or(Err(Halt::RetryLater))?;
        }
        Ok(())
    }

    async fn send_thread_share(&self, login: &HubLogin, open: PendingOpen<'_>) -> Result<(), Halt> {
        let sent = match open.send().await {
            Ok(sent) => sent,
            Err(refused) => return self.unshared(login, *refused).await,
        };
        clear_off(&self.queue, login.user_id()).await;
        for org_slug in sent.skipped() {
            self.warn_once_of_skipped_org(login, org_slug).await;
        }
        Ok(sent.settle().await?)
    }

    /// Settles an open Octavo didn't take as its answer says.
    async fn unshared(
        &self,
        login: &HubLogin,
        NotSent { pending, error }: NotSent<PendingOpen<'_>>,
    ) -> Result<(), Halt> {
        let user = login.user_id();
        match error {
            HubCallError::ThreadNotStored(_) => {
                // Octavo refuses an account with it off before it looks for the thread, so this
                // answer finds Octavo on, as a stored share does.
                clear_off(&self.queue, user).await;
                pending.expire(ORPHAN_AFTER).await?;
                Ok(())
            }
            HubCallError::OctavoOff(_) => {
                tracing::debug!("octavo is off for the account; leaving its thread shares queued");
                mark_off(&self.queue, user).await;
                Err(Halt::Off)
            }
            error if error.is_permanent() => {
                let orgs = pending.orgs();
                tracing::warn!(?error, ?orgs, "octavo refused to share a thread; dropping it");
                Ok(pending.give_up().await?)
            }
            error => {
                tracing::warn!(?error, "failed to share a thread on octavo; retrying later");
                Err(Halt::RetryLater)
            }
        }
    }

    /// Splits `messages` into those that may go up now and the ids of those that must wait (see
    /// [`gate`](super::gate)).
    async fn clear(&self, login: &HubLogin, messages: Vec<Message>) -> Result<Cleared, Halt> {
        let rows = Rows::new(&self.queue, login.user_id(), messages);
        let closing = rows.record_dirs(&self.configs).await?.narrow().await?;
        let sent =
            Closes::of(&self.queue, login).send(tokio::time::Instant::now() + PASS_BUDGET).await?;
        Ok(closing.clear(sent).await?)
    }

    async fn warn_once_of_skipped_org(&self, login: &HubLogin, org_slug: &str) {
        let news = self.queue.note_skipped_org(login.user_id(), org_slug).await;
        // Unable to tell, it warns: a repeat beats never saying so.
        if news.unwrap_or_else(|err| {
            tracing::warn!(?err, "failed to note a skipped organization");
            true
        }) {
            tracing::warn!(
                org = org_slug,
                "a config file's orgs names an organization this account isn't a member of; not \
                 sharing threads with it"
            );
        }
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
            // A series gone since the backfill was asked for stays unfixed, and unwalked, until
            // it is back.
            if let Some(tail) = self.thread_tail(host_id).await? {
                tails.push((host_id, tail));
            }
        }

        self.queue.fix_thread_backfills(user, tails).await.map_err(|err| {
            tracing::warn!(?err, "failed to fix the octavo thread backfills; retrying later");
            RetryLater
        })
    }

    /// Uploads a page of `login`'s first backfill not done, and moves it on past the page.
    async fn backfill(&self, login: &HubLogin) -> Result<NextPass, Halt> {
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
            // Read before the page. Sync only appends past the tail, so a gap below a tail read
            // first never fills. A tail read after the page could take in records sync stored in
            // between.
            let tail = self.thread_tail(host_id).await?;
            let series = RecordSeriesKey::new(host_id, RecordTag::AiSession);
            let read = self.records.page(&series, next_idx..until_idx, BACKFILL_PAGE);
            let page = read.await.map_err(|err| {
                tracing::warn!(?err, "failed to read a thread backfill; retrying later");
                RetryLater
            })?;

            // Nothing is stored in what is left of it. Places past the tail wait for sync, which
            // downloads a series in order. A gap below the tail is done.
            let Some(&(last, _, _)) = page.last() else {
                if tail.is_some_and(|tail| tail >= until_idx) {
                    self.advance_backfill(login, host_id, until_idx).await?;
                }
                continue;
            };
            let sent = self.send_backfill_page(login, page).await?;
            match sent {
                Uploaded::Stored => {
                    clear_off(&self.queue, user).await;
                }
                Uploaded::NoneStored | Uploaded::Nothing => {}
                Uploaded::Off => {
                    tracing::debug!("octavo is off for the account; stopping its thread backfill");
                    mark_off(&self.queue, user).await;
                    // Left where it is, so it goes on once the account has Octavo again.
                    return Err(Halt::Off);
                }
            }
            self.advance_backfill(login, host_id, last + 1).await?;
            if sent == Uploaded::NoneStored {
                self.halt_if_off(login).await?;
            }
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
    ) -> Result<Uploaded, UploadRetryLater> {
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
        // Those that must wait go to the queue, which looks at them again, so the backfill moves on.
        let Cleared { ready, waiting } = match self.clear(login, messages).await {
            Ok(cleared) => cleared,
            Err(Halt::Off) => return Ok(Uploaded::Off),
            Err(Halt::RetryLater) => return Err(UploadRetryLater { stored: false }),
        };

        let queued = async {
            self.queue.push_thread_messages(user, waiting).await?;
            self.queue.push_thread_messages(user, held.iter().copied()).await?;
            self.queue.hold_thread_messages(user, held).await
        };
        queued.await.map_err(|err| {
            tracing::warn!(?err, "failed to queue unreadable thread messages; retrying later");
            UploadRetryLater { stored: false }
        })?;

        upload_thread_messages(login, converted(ready).await).await
    }

    /// The idx of the last record stored of `host`'s thread series, if it has one.
    async fn thread_tail(&self, host: HostId) -> Result<Option<RecordIdx>, RetryLater> {
        let series = RecordSeriesKey::new(host, RecordTag::AiSession);
        let tail = self.handle.store().last(&series).await.map_err(|err| {
            tracing::warn!(?err, "failed to read a thread series' tail; retrying later");
            RetryLater
        })?;
        Ok(tail.map(|tail| tail.idx))
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

    async fn upload_output(&self, login: &HubLogin, id: HistoryId) -> Result<(), Halt> {
        // Octavo can't have an entry `history end` hasn't stored here; its upload wakes the engine.
        if !matches!(self.handle.history_db().load(id).await, Ok(Some(_))) {
            return Ok(self.expire_output(login, id).await?);
        }

        let capture = match self.outputs.get(id).await {
            Ok(capture) => capture,
            Err(err) => {
                tracing::warn!(?err, %id, "failed to read a queued output; skipping it");
                return Ok(self.expire_output(login, id).await?);
            }
        };
        // Deleted or collected locally, or not stored yet. A late output whose insert lands
        // between `outputs.get` and `remove_output` is lost: two local statements wide, and only
        // on the re-enable path.
        let Some(capture) = capture else {
            return Ok(self.dequeue_output(login, id).await?);
        };

        match login.upload_output(id, capture).await {
            Ok(()) => {
                clear_off(&self.queue, login.user_id()).await;
            }
            Err(HubCallError::HistoryNotStored(_)) => {
                // Octavo refuses an account with it off before it looks for the entry, so this
                // answer finds Octavo on, as a stored output does.
                clear_off(&self.queue, login.user_id()).await;
                tracing::debug!(%id, "octavo has not stored this output's entry yet");
                return Ok(self.expire_output(login, id).await?);
            }
            Err(HubCallError::OctavoOff(_)) => {
                tracing::debug!(%id, "octavo is off for the account; leaving this output queued");
                mark_off(&self.queue, login.user_id()).await;
                return Err(Halt::Off);
            }
            Err(err) if err.is_permanent() => {
                tracing::warn!(?err, %id, "octavo will never accept this output; dropping it");
                self.dequeue_output(login, id).await?;
                return self.halt_if_off(login).await;
            }
            Err(err) => {
                tracing::warn!(?err, %id, "failed to upload an output to octavo; retrying later");
                return Err(Halt::RetryLater);
            }
        }

        Ok(self.dequeue_output(login, id).await?)
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

    /// Ends the pass, as [`Halt::Off`], while `login`'s account is marked off: Octavo answered an
    /// upload without storing it, which doesn't say whether the account has Octavo again.
    async fn halt_if_off(&self, login: &HubLogin) -> Result<(), Halt> {
        if is_off(&self.queue, login.user_id()).await {
            return Err(Halt::Off);
        }
        Ok(())
    }
}

/// `messages` as Octavo takes them, less those it never will, which are dropped with a warning.
impl From<QueueError> for Halt {
    fn from(err: QueueError) -> Self {
        tracing::warn!(?err, "failed to read or write the octavo upload queue; retrying later");
        Self::RetryLater
    }
}

impl From<CloseHalt> for Halt {
    fn from(halt: CloseHalt) -> Self {
        match halt {
            CloseHalt::Off => Self::Off,
            CloseHalt::Queue(err) => err.into(),
        }
    }
}

pub(super) async fn converted(messages: Vec<Message>) -> Vec<pb::CapturedThreadMessage> {
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

/// How [`upload`] left the items it was given.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Uploaded {
    /// Each went up, or was refused and dropped, and Octavo stored at least one request.
    Stored,
    /// Each was refused and dropped.
    ///
    /// This says nothing of the account's Octavo: the server checks a request's items before
    /// whether their owner has it.
    NoneStored,
    /// There were none, so Octavo wasn't asked.
    Nothing,
    /// The account has Octavo off, so none past the refused request went.
    Off,
}

/// Uploads `messages`, dropping with a warning each one Octavo refuses.
pub(super) async fn upload_thread_messages(
    login: &HubLogin,
    messages: Vec<pb::CapturedThreadMessage>,
) -> Result<Uploaded, UploadRetryLater> {
    upload(
        messages,
        "thread messages",
        |batch| login.upload_thread_messages(batch),
        |message, err| {
            tracing::warn!(
                ?err,
                id = ?record_id(message),
                "octavo will never accept this thread message; dropping it"
            );
        },
    )
    .await
}

/// Uploads `items`, `what` they are, with `send` a batch at a time, dropping each one Octavo
/// refuses once `refused` is told of it.
///
/// A request Octavo refuses, or that runs out of time, goes again in halves, so one too large for
/// the server or the uplink shrinks instead of repeating; a lone item that runs out of time is
/// retried later.
pub(super) async fn upload<M, Fut>(
    items: Vec<M>,
    what: &str,
    send: impl Fn(pb::Batch<M>) -> Fut,
    mut refused: impl FnMut(&M, &HubCallError),
) -> Result<Uploaded, UploadRetryLater>
where
    M: prost::Message + Clone,
    Fut: Future<Output = Result<(), HubCallError>>,
{
    if items.is_empty() {
        return Ok(Uploaded::Nothing);
    }

    let mut sent = Uploaded::NoneStored;
    for batch in pb::Batch::batches(items) {
        let mut requests = vec![batch];
        while let Some(request) = requests.pop() {
            let Err(err) = send(request.clone()).await else {
                sent = Uploaded::Stored;
                continue;
            };
            match (UploadFailure::of(&err), request.halves()) {
                (UploadFailure::Off, _) => return Ok(Uploaded::Off),
                (UploadFailure::Transient, _) | (UploadFailure::TimedOut, Err(_)) => {
                    tracing::warn!(?err, "failed to upload {what} to octavo; retrying later");
                    return Err(UploadRetryLater {
                        stored: sent == Uploaded::Stored,
                    });
                }
                (UploadFailure::Refused, Err(lone)) => {
                    for item in lone.items() {
                        refused(item, &err);
                    }
                }
                (UploadFailure::Refused | UploadFailure::TimedOut, Ok((front, back))) => {
                    // The front half is popped first, which keeps the items in order.
                    requests.extend([back, front]);
                }
            }
        }
    }

    Ok(sent)
}

/// Why a batched request failed, as [`upload`] acts on it.
enum UploadFailure {
    /// The account has Octavo off, so it stores none of them for now.
    Off,
    /// Octavo will never store one of its messages.
    Refused,
    /// It didn't go up inside `network_timeout`, which a smaller request may.
    TimedOut,
    /// It may go up as it is on a later try.
    Transient,
}

impl UploadFailure {
    fn of(err: &HubCallError) -> Self {
        if let HubCallError::OctavoOff(_) = err {
            return Self::Off;
        }
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

fn history_id(entry: &pb::History) -> Option<HistoryId> {
    let id = entry.id.as_ref()?;
    Uuid::from_slice(&id.value).ok().map(HistoryId::new)
}

/// The id of the record `message` was converted from.
fn record_id(message: &pb::CapturedThreadMessage) -> Option<Uuid> {
    let id = message.message.as_ref()?.id.as_ref()?;
    Uuid::from_slice(&id.value).ok()
}
