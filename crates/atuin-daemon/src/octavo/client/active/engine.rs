use std::collections::HashSet;
use std::num::NonZeroU32;
use std::ops::ControlFlow;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use atuin_client::ai_session::{AiSessionStore, Loaded, Message};
use atuin_client::record::decode::map_parallel;
use atuin_common::futures::{Backoff, Debounce, Delays};
use atuin_domain::record::{HostId, RecordId, RecordIdx, RecordSeriesKey, RecordTag};
use atuin_octavo::hub::{HubCallError, HubClient, HubLogin};
use atuin_octavo::pb;
use atuin_octavo::queue::{
    NotSent, PendingOpen, PendingThreadMessageUpload, QueueError, ThreadBackfill, UploadQueue,
};
use futures::StreamExt;
use tokio::sync::Notify;
use tokio::sync::broadcast::Receiver;
use tokio::sync::broadcast::error::RecvError;
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tonic::Code;
use uuid::Uuid;

use super::gate::{Cleared, CloseHalt, Closes, Opening, Rows};
use super::starts::{Goes, Sorted, Starts};
use super::{clear_off, is_off, mark_off};
use crate::daemon::DaemonHandle;
use crate::events::DaemonEvent;
use crate::octavo::threads_on;

const POLL_INTERVAL: Duration = Duration::from_secs(5 * 60);
const BACKOFF: Backoff = Backoff::Exponential {
    initial: Duration::from_secs(5),
    max: Duration::from_secs(30 * 60),
    factor: NonZeroU32::new(2).unwrap(),
};

/// How long a queued thread message or a queued thread share waits to reach Octavo before it's
/// dropped.
///
/// A thread message waits while its record isn't stored: capture queues it just before the push,
/// which may fail. A thread share waits while its thread isn't stored, whose messages normally go
/// ahead of it, so that thread's were refused or dropped, or the user deleted it; its next message
/// queues the share afresh.
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

/// How long one pass can spend on thread messages or thread shares, each, so neither holds up the
/// rest for long.
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
        records: AiSessionStore,
        wake: Arc<Notify>,
        starts: Starts,
    ) -> Self {
        let events = handle.subscribe();
        let inner = EngineInner {
            queue,
            hub,
            handle,
            records,
            wake,
            logins: Notify::new(),
            logins_heard: AtomicU64::new(0),
            starts,
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
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum NextPass {
    /// Once something wakes the engine, at a login, at the poll interval, or once background work
    /// is through a backoff.
    Waiting,
    /// At once: more backfill exists, or threads were left to look up.
    Now,
}

impl NextPass {
    /// When the next pass goes after work that asked for `self`, then work that asked for `after`:
    /// at once if either has more.
    const fn and(self, after: Self) -> Self {
        match (self, after) {
            (Self::Waiting, Self::Waiting) => Self::Waiting,
            (Self::Now, _) | (_, Self::Now) => Self::Now,
        }
    }
}

/// The threads of a login that are queued to be opened and haven't been sent yet.
#[derive(Debug)]
struct HeldShares {
    /// Those that the config files keep here, or that wait for a lookup.
    held: HashSet<pb::HarnessSession>,
    /// Threads that another host captured. Their shares are given up, because only that host can
    /// read the config files that decide them.
    elsewhere: HashSet<pb::HarnessSession>,
    /// When the next pass goes: at once if a lookup was cut off.
    next: NextPass,
}

/// What a pass that sent its live work hands on to background work.
struct Sending {
    login: HubLogin,
    /// When the next pass goes for the live work: at once if it left threads to look up.
    live: NextPass,
}

/// Whether background work goes in a pass: it does unless it's waiting out a [`BACKOFF`] of its
/// own, after a failure that may pass. A login cuts that short, as a new session may let it
/// through.
#[derive(Debug, Default)]
struct Background {
    backing_off: Option<BackingOff>,
}

#[derive(Debug)]
struct BackingOff {
    until: Instant,
    /// The delays of the failures after.
    delays: Delays,
    /// [`EngineInner::logins_heard`] as the work that failed went.
    logins_heard: u64,
}

impl Background {
    fn is_due(&self, now: Instant, logins_heard: u64) -> bool {
        self.backing_off
            .as_ref()
            .is_none_or(|off| now >= off.until || off.logins_heard != logins_heard)
    }

    /// Waits out the backoff, or forever once it's over: a pass that sends no background work,
    /// such as one with no login, leaves it in place.
    async fn backed_off(&self) {
        match &self.backing_off {
            Some(off) if off.until > Instant::now() => tokio::time::sleep_until(off.until).await,
            Some(_) | None => std::future::pending().await,
        }
    }

    fn went(&mut self) {
        self.backing_off = None;
    }

    /// Backs off after a failure of work that went at `now`, with `logins_heard` then. The
    /// delays start over after a login.
    fn failed(&mut self, now: Instant, logins_heard: u64) {
        let mut delays = match self.backing_off.take() {
            Some(off) if off.logins_heard == logins_heard => off.delays,
            Some(_) | None => BACKOFF.delays(),
        };
        self.backing_off = Some(BackingOff {
            until: now + delays.next_delay(),
            delays,
            logins_heard,
        });
    }
}

struct EngineInner {
    queue: UploadQueue,
    hub: HubClient,
    handle: DaemonHandle,
    /// Where queued thread messages are read from.
    records: AiSessionStore,
    wake: Arc<Notify>,
    /// Notified at each `atuin login`, which sends a pass that failed, or one waiting, at once.
    logins: Notify,
    /// How many logins the engine has heard of, so background work backing off can tell one came.
    logins_heard: AtomicU64,
    /// Who threads are shared with, and which threads go up.
    starts: Starts,
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
        let mut background = Background::default();
        loop {
            // Only a woken pass waits for quiet; a retry waits out its backoff instead.
            if woken {
                DEBOUNCE.settle(|| self.wake.notified()).await;
            }
            let next = match self.send_live(woken).await {
                Some(sending) => {
                    let next = self.send_background(&sending, &mut background).await;
                    sending.live.and(next)
                }
                None => NextPass::Waiting,
            };

            woken = match next {
                NextPass::Now => false,
                NextPass::Waiting => tokio::select! {
                    () = self.wake.notified() => true,
                    () = self.logins.notified() => false,
                    () = tokio::time::sleep(POLL_INTERVAL) => false,
                    () = background.backed_off() => false,
                },
            };
        }
    }

    /// Sends the live work [`Self::send_pending`] sends, retrying with [`BACKOFF`] until it goes,
    /// and returns what background work goes on with, if any.
    async fn send_live(&self, woken: bool) -> Option<Sending> {
        // Only a login cuts the backoff short, as a new session may let the failed pass through;
        // what a wake brings waits for the next attempt. The pass after a login probes a paused
        // queue, as one the poll starts does. Atomic only so this future stays `Send`: both
        // closures run on this task.
        let retry_woken = AtomicBool::new(woken);
        BACKOFF
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
            .await
    }

    /// Sends `sending`'s background work unless it's waiting out a backoff, which a failure that
    /// may pass starts. A wake doesn't cut that backoff short, so the next passes send their live
    /// work while it lasts.
    async fn send_background(&self, sending: &Sending, background: &mut Background) -> NextPass {
        // Read before the work, so a login while it fails sends it again at once.
        let logins = self.logins_heard.load(Ordering::Relaxed);
        if !background.is_due(Instant::now(), logins) {
            return NextPass::Waiting;
        }

        match self.backfill(&sending.login).await {
            Ok(next) => {
                background.went();
                next
            }
            Err(Halt::Off) => {
                background.went();
                NextPass::Waiting
            }
            Err(Halt::RetryLater) => {
                background.failed(Instant::now(), logins);
                NextPass::Waiting
            }
        }
    }

    /// Passes each login the daemon hears of on to [`Self::logins`], and wakes the engine at each
    /// settings reload.
    async fn hear_events(&self, mut events: Receiver<DaemonEvent>) {
        loop {
            match events.recv().await {
                // The events a lag dropped may have held one.
                Ok(DaemonEvent::LoggedIn) | Err(RecvError::Lagged(_)) => {
                    self.logins_heard.fetch_add(1, Ordering::Relaxed);
                    self.logins.notify_one();
                }
                // `atuin setup --hub` clears the off mark before it writes the config.
                Ok(DaemonEvent::SettingsReloaded) => self.wake.notify_one(),
                Ok(DaemonEvent::ShutdownRequested) => {}
                Err(RecvError::Closed) => return,
            }
        }
    }

    /// Sends the logged-in user's queued thread messages and thread shares while Octavo is on
    /// here, returning what [`Self::backfill`] goes on with, if it may. What other users queued
    /// waits for them to log in again.
    ///
    /// While the account is marked off, a woken pass sends none of the uploads, and any other is a
    /// probe: it goes on past the first upload Octavo answers only if the answer finds Octavo on.
    async fn send_pending(&self, woken: bool) -> Result<Option<Sending>, RetryLater> {
        if !threads_on(self.starts.configs()).await {
            return Ok(None);
        }

        let settings = self.handle.settings().await.clone();
        let login = match self.hub.login(&settings).await {
            Ok(login) => login,
            Err(HubCallError::NotLoggedIn) => return Ok(None),
            Err(err) => {
                tracing::warn!(?err, "failed to log in to octavo; retrying later");
                return Err(RetryLater);
            }
        };

        // Before the bounds are fixed, so that only this host's backfill gets a bound and counts in
        // setup's gauge.
        self.drop_other_hosts_backfills(&login).await?;
        // Fixed even while the account is marked off: what capture lets by then goes up direct or
        // not at all, and a bound fixed once the mark clears would take it.
        self.fix_backfill_bounds(&login).await?;
        // A woken pass leaves a paused queue be. Any other probes it, and the poll starts one at most
        // `POLL_INTERVAL` after the last wake: a stale mark lasts that long, for one refused round
        // trip a poll while the account is off.
        if woken && is_off(&self.queue, login.user_id()).await {
            return Ok(None);
        }

        match self.send_live_uploads(&login).await {
            Ok(live) => Ok(Some(Sending { login, live })),
            Err(Halt::Off) => Ok(None),
            Err(Halt::RetryLater) => Err(RetryLater),
        }
    }

    /// Sends `login`'s queued thread messages, then their queued thread shares, for up to
    /// [`PASS_BUDGET`], all up to the first [`Halt::Off`], saying whether threads were left to
    /// look up.
    async fn send_live_uploads(&self, login: &HubLogin) -> Result<NextPass, Halt> {
        let threads = self.send_threads(login).await?;
        // Last, as a share waits for its thread's messages. One that may go later waits for the
        // next pass rather than backing off: a wake can't cut a backoff short, so the thread
        // messages it brings would wait it out too.
        let shares = match self.send_thread_shares(login).await {
            Ok(next) => next,
            Err(Halt::Off) => return Err(Halt::Off),
            Err(Halt::RetryLater) => NextPass::Waiting,
        };
        Ok(threads.and(shares))
    }

    /// Sends the queued thread messages of `login`'s user, as the config files say which go up.
    /// Those of threads not looked up within [`PASS_BUDGET`] stay queued, for the next pass to go
    /// at once.
    async fn send_threads(&self, login: &HubLogin) -> Result<NextPass, Halt> {
        let until = Instant::now() + PASS_BUDGET;
        let mut next = NextPass::Waiting;
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
            next = next.and(self.send_thread_page(login, &ids, until).await?);
        }
        Ok(next)
    }

    /// Uploads the queued thread messages `ids`, and settles each: dequeued once uploaded or
    /// refused, or once the config files keep its thread here, held while this build can't read its
    /// record, and expired while its record isn't stored. Those to upload stay queued while
    /// Octavo refuses them as off for the account, as do those of threads not looked up by
    /// `until`, for the next pass to go at once.
    async fn send_thread_page(
        &self,
        login: &HubLogin,
        ids: &[RecordId],
        until: Instant,
    ) -> Result<NextPass, Halt> {
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
        // Queued under another config, those that the config files keep here now stay here.
        let Sorted {
            going,
            waiting: unsure,
            overdue,
        } = self.starts.uploads_stored(messages, until).await;
        let next = if overdue {
            NextPass::Now
        } else {
            NextPass::Waiting
        };
        // Those that must wait stay queued for the next pass.
        let Cleared { ready, waiting } = self.clear(login, going).await?;
        let waiting: HashSet<_> = waiting.into_iter().chain(unsure).collect();
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
            self.halt_if_off(login).await?;
        }
        Ok(next)
    }

    /// Sends `login`'s queued thread shares for up to [`PASS_BUDGET`], up to the first that fails
    /// for a reason that may pass, as one the budget cuts short does. A share of a thread that
    /// the config files keep here stays queued. A share of a thread that another host captured is
    /// given up.
    ///
    /// A share cut short goes again, which Octavo takes as a repeat even if it stored the first.
    /// Says whether threads were left to look up.
    async fn send_thread_shares(&self, login: &HubLogin) -> Result<NextPass, Halt> {
        // Before the budget starts, as each thread's config files may take a lookup of their own,
        // with a budget of its own.
        let shares = self.held_shares(login, Instant::now() + PASS_BUDGET).await?;
        let opening = Opening::of(&self.queue, login).await?.without(&shares.elsewhere);
        let narrowed = opening.look_again(self.starts.configs()).await?.narrow().await?;
        // First, so no share opens to an organization one of its thread's directories keeps out.
        let deadline = Instant::now() + PASS_BUDGET;
        let sent = Closes::of(&self.queue, login).send(deadline).await?;
        for open in narrowed.pending(sent, shares.held).await? {
            if shares.elsewhere.contains(open.thread()) {
                open.give_up().await?;
                continue;
            }
            let sent = tokio::time::timeout_at(deadline, self.send_thread_share(login, open)).await;
            sent.unwrap_or(Err(Halt::RetryLater))?;
        }
        Ok(shares.next)
    }

    /// The threads of `login` that are queued to be opened, split into those whose shares wait
    /// and those whose shares are given up.
    ///
    /// A thread waits when its config files keep it here, or when its lookup didn't finish by
    /// `until`. In that case the next pass goes at once.
    async fn held_shares(&self, login: &HubLogin, until: Instant) -> Result<HeldShares, Halt> {
        let mut shares = HeldShares {
            held: HashSet::new(),
            elsewhere: HashSet::new(),
            next: NextPass::Waiting,
        };
        for thread in self.queue.opening_threads(login.user_id()).await? {
            match self.starts.shares(&thread, until).await {
                Goes::Up => {}
                Goes::Overdue => {
                    shares.held.insert(thread);
                    shares.next = NextPass::Now;
                }
                Goes::Here | Goes::Later => {
                    shares.held.insert(thread);
                }
                Goes::CapturedElsewhere => {
                    shares.elsewhere.insert(thread);
                }
            }
        }
        Ok(shares)
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
        let closing = rows.record_dirs(self.starts.configs()).await?.narrow().await?;
        let sent = Closes::of(&self.queue, login).send(Instant::now() + PASS_BUDGET).await?;
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

    /// Drops the backfills of other hosts' series from `login`'s queue.
    ///
    /// Each machine uploads only the threads that it captured, because uploading another
    /// machine's backfill would judge its threads by this machine's config files.
    async fn drop_other_hosts_backfills(&self, login: &HubLogin) -> Result<(), RetryLater> {
        self.queue
            .drop_other_hosts_thread_backfills(login.user_id(), self.records.host_id())
            .await
            .map_err(|err| {
                tracing::warn!(
                    ?err,
                    "failed to drop the octavo thread backfills of other hosts; retrying later"
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

    /// Uploads `page` of a backfill but for the threads the config files keep here, and queues its
    /// records this build can't read, held, for a restarted daemon to upload.
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
        // Each page goes in a pass of its own, so those a lookup's deadline cuts off are queued
        // like the rest that wait.
        let Sorted {
            going,
            waiting: unsure,
            ..
        } = self.starts.uploads_stored(messages, Instant::now() + PASS_BUDGET).await;
        // Those that must wait go to the queue, which looks at them again, so the backfill moves on.
        let Cleared { ready, waiting } = match self.clear(login, going).await {
            Ok(cleared) => cleared,
            Err(Halt::Off) => return Ok(Uploaded::Off),
            Err(Halt::RetryLater) => return Err(UploadRetryLater { stored: false }),
        };

        let queued = async {
            self.queue.push_thread_messages(user, waiting.into_iter().chain(unsure)).await?;
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

/// The id of the record `message` was converted from.
fn record_id(message: &pb::CapturedThreadMessage) -> Option<Uuid> {
    let id = message.message.as_ref()?.id.as_ref()?;
    Uuid::from_slice(&id.value).ok()
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    /// Live work that left threads to look up has the next pass go at once, however the
    /// background work after it went.
    #[rstest]
    #[case::neither_has_more(NextPass::Waiting, NextPass::Waiting, NextPass::Waiting)]
    #[case::the_live_work_has_more(NextPass::Now, NextPass::Waiting, NextPass::Now)]
    #[case::the_background_has_more(NextPass::Waiting, NextPass::Now, NextPass::Now)]
    fn the_next_pass_goes_as_soon_as_any_work_asks(
        #[case] live: NextPass,
        #[case] background: NextPass,
        #[case] expected: NextPass,
    ) {
        assert_eq!(live.and(background), expected);
    }

    /// The first delay of [`BACKOFF`], and the factor of each after, before jitter.
    fn backoff() -> (Duration, u32) {
        let Backoff::Exponential {
            initial, factor, ..
        } = BACKOFF
        else {
            unreachable!("BACKOFF is exponential");
        };
        (initial, factor.get())
    }

    /// `delay` less, then more, than its jitter.
    fn around(delay: Duration) -> [Duration; 2] {
        [delay * 8 / 10, delay * 12 / 10]
    }

    #[rstest]
    fn background_backs_off_further_at_each_failure() {
        let (initial, factor) = backoff();
        let start = Instant::now();
        let mut background = Background::default();
        assert!(background.is_due(start, 0));

        background.failed(start, 0);
        let [early, late] = around(initial);
        assert!(!background.is_due(start + early, 0));
        assert!(background.is_due(start + late, 0));

        let retried = start + late;
        background.failed(retried, 0);
        let [early, late] = around(initial * factor);
        assert!(!background.is_due(retried + early, 0));
        assert!(background.is_due(retried + late, 0));
    }

    #[rstest]
    fn a_login_ends_the_backoff_and_starts_its_delays_over() {
        let (initial, _) = backoff();
        let start = Instant::now();
        let mut background = Background::default();
        background.failed(start, 0);
        background.failed(start, 0);
        assert!(background.is_due(start, 1));

        background.failed(start, 1);
        let [early, late] = around(initial);
        assert!(!background.is_due(start + early, 1));
        assert!(background.is_due(start + late, 1));
    }

    #[rstest]
    fn going_ends_the_backoff_and_starts_its_delays_over() {
        let (initial, _) = backoff();
        let start = Instant::now();
        let mut background = Background::default();
        background.failed(start, 0);
        background.failed(start, 0);
        background.went();
        assert!(background.is_due(start, 0));

        background.failed(start, 0);
        assert!(background.is_due(start + around(initial)[1], 0));
    }

    /// Once over, a backoff no pass took up must not end every wait after it, or a run loop with
    /// no login spins.
    #[tokio::test(start_paused = true)]
    async fn a_backoff_ends_one_wait() {
        let mut background = Background::default();
        background.failed(Instant::now(), 0);

        let first = tokio::time::timeout(POLL_INTERVAL, background.backed_off()).await;
        assert!(first.is_ok(), "the backoff never ran out");
        let second = tokio::time::timeout(POLL_INTERVAL, background.backed_off()).await;
        assert!(second.is_err(), "the backoff ended a second wait");
    }
}
