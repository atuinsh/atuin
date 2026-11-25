//! Turns backend events into activity segments, debounced snapshots, dialogs, and
//! notifications.
//!
//! A [`Capture`] follows the active window of a [`Backend`] and yields [`Record`]s. It also
//! reads every other window once when it starts, and again when the window changes. Storing
//! the records is up to the caller. Capture reads nothing while the screen is locked, and reads
//! every window again once the screen unlocks.
//!
// `Capture` schedules the work of three parts, which share a `Context`:
//
// - `active`: the active window's segments and snapshots, which `debounce` times.
// - `dialogs`: the dialogs and notification windows that appear.
// - `background`: the windows in the background, and the sweep of every window.
//
// `notifications` hands out the notifications that the platform delivers apart from windows,
// as soon as they arrive. `scrubber` cleans everything that capture hands out, and `pacing`
// holds the rests and retries that the three parts share.

mod active;
mod background;
mod config;
mod context;
mod debounce;
mod dialogs;
mod notifications;
mod pacing;
mod record;
mod scrubber;
mod title;

#[cfg(test)]
mod tests;

use std::time::Duration;

use active::{Active, Followed};
use background::{BackgroundWindows, Left};
pub use config::{AppFilter, BackgroundReads, Config, Scope, Timing};
use context::Context;
use dialogs::{Announcement, Dialogs};
pub use record::{
    Background, Change, Content, ContentHash, Dialog, EncodedTree, HashMismatch, Notification,
    Record, Segment, SegmentId, Snapshot, SnapshotContent, Trigger, WindowId,
};
pub use scrubber::Redact;
use tokio::time::Instant;
use tracing::debug;

use crate::backend::{Backend, Event, EventCount, Limits, Screen, WindowClass};

/// What [`Capture::stop`] does with an edit in the followed window that waits to be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitingEdit {
    /// Reads the window once more with the configuration that capture runs with.
    Read,
    /// Drops the edit. A stop that comes with a new configuration drops the edit, because the
    /// new configuration may leave the window out.
    Discard,
}

/// The longest that the read of a waiting edit can hold up [`Capture::stop`].
///
/// When a daemon stops a helper, the daemon gives the helper a couple of seconds to finish, and
/// the segment must end within that time.
const STOP_READ_TIME: Duration = Duration::from_secs(1);

/// The budget of the read of a waiting edit when capture stops. It leaves the backend time to
/// hand back what it read before [`STOP_READ_TIME`] runs out.
const STOP_READ_BUDGET: Duration = Duration::from_millis(500);

/// The window and segment ids that a capture hands out next.
///
/// A capture that continues from the ids of an earlier capture over the same backend never
/// reuses an id of the earlier one, so a store can keep the ids apart for as long as the
/// backend lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NextIds {
    window: u64,
    segment: u64,
}

impl Default for NextIds {
    fn default() -> Self {
        Self {
            window: 1,
            segment: 1,
        }
    }
}

/// What a stopped [`Capture`] gives back, for a new capture to continue from.
pub struct Parts<B> {
    pub backend: B,
    /// The ids that the next capture over `backend` starts from.
    pub ids: NextIds,
}

/// What [`Capture::into_stopped`] gives back: the parts for a new capture, and the
/// configuration of the capture that stopped.
pub struct Stopped<B> {
    pub parts: Parts<B>,
    pub config: Config,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Idle,
    Running,
    Stopped,
}

/// The work that capture does when the work is due. When several kinds of work are due, the
/// earlier variant goes first.
enum Work {
    /// Look at a window or an application that appeared.
    Arrival,
    /// Ask the current application again whether it has an active window.
    Handover,
    /// Follow a window whose activation arrived while capture declined the window, when the
    /// window is still active.
    MissedActivation,
    /// Activate a window again when its class was unknown at its last activation and the window is
    /// still active.
    Unclassified,
    Snapshot,
    /// Read the background window whose changes are due first.
    Background,
    Sweep,
}

/// What [`Capture::next_output`] yields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Output {
    Record(Record),
    /// The screen locked or unlocked. Capture yields every state that the backend reports,
    /// after the records that capture handed out before the change.
    Screen(Screen),
}

/// Work and the time when it is due.
struct Due {
    at: Instant,
    work: Work,
}

/// A capture session over one backend.
pub struct Capture<B: Backend> {
    ctx: Context<B>,
    active: Active<B>,
    dialogs: Dialogs<B>,
    background: BackgroundWindows<B>,
    phase: Phase,
    screen: Screen,
    /// The state of the screen that waits to be handed out after the records.
    announced: Option<Screen>,
}

impl<B: Backend> Capture<B> {
    /// Starts a capture whose ids start over.
    pub fn new(backend: B, config: Config) -> Self {
        Self::resume(
            Parts {
                backend,
                ids: NextIds::default(),
            },
            config,
        )
    }

    /// Starts a capture over the backend of a stopped capture, which continues the stopped
    /// capture's ids.
    pub fn resume(parts: Parts<B>, config: Config) -> Self {
        let Parts { backend, ids } = parts;
        Self {
            active: Active::new(config.timing, ids.segment),
            dialogs: Dialogs::new(),
            background: BackgroundWindows::new(config.background),
            ctx: Context::new(backend, config, ids.window),
            phase: Phase::Idle,
            screen: Screen::Unlocked,
            announced: None,
        }
    }

    /// Makes capture take credentials out of everything that it hands out from now on.
    #[must_use]
    pub fn with_redactor(mut self, redactor: impl Redact + 'static) -> Self {
        self.ctx.scrubber.set_redactor(redactor);
        self
    }

    pub const fn backend(&self) -> &B {
        &self.ctx.backend
    }

    /// Waits for the next record, and skips the changes of the screen. Returns nothing once
    /// capture has stopped.
    ///
    /// Capture stops when the backend does, or after [`stop`](Self::stop). Dropping the
    /// returned future can lose the window event that capture was handling, so drop it only
    /// in order to stop.
    pub async fn next(&mut self) -> Option<Record> {
        loop {
            if let Output::Record(record) = self.next_output().await? {
                return Some(record);
            }
        }
    }

    /// Waits for the next record or change of the screen. Returns nothing once capture has
    /// stopped.
    ///
    /// Dropping the returned future can lose the window event that capture was handling, as
    /// with [`next`](Self::next).
    pub async fn next_output(&mut self) -> Option<Output> {
        loop {
            if let Some(record) = self.ctx.records.pop_front() {
                return Some(Output::Record(record));
            }
            if let Some(screen) = self.announced.take() {
                return Some(Output::Screen(screen));
            }
            match self.phase {
                Phase::Stopped => return None,
                Phase::Idle => {
                    self.phase = Phase::Running;
                    self.follow_desktop().await;
                }
                Phase::Running => {
                    let schedule = self.schedule();
                    let wake = schedule.iter().flatten().map(|due| due.at).min();
                    tokio::select! {
                        event = self.ctx.backend.next() => match event {
                            Some(event) => self.handle(event).await,
                            None => self.stop(WaitingEdit::Read).await,
                        },
                        () = sleep_until(wake) => {
                            if let Some(work) = first_due(schedule, Instant::now()) {
                                self.perform(work).await;
                            }
                        }
                    }
                }
            }
        }
    }

    /// Follows the window that is active, and schedules the sweep of every window.
    async fn follow_desktop(&mut self) {
        if let Some(window) = self.ctx.backend.active_window().await {
            self.activate(window).await;
        }
        self.background.start_sweep(&self.ctx).await;
    }

    /// The work that waits, in order of priority. Nothing waits while the screen is locked.
    fn schedule(&self) -> [Option<Due>; 7] {
        let due = |at: Option<Instant>, work| {
            at.filter(|_| self.screen == Screen::Unlocked).map(|at| Due { at, work })
        };
        [
            due(self.dialogs.due(), Work::Arrival),
            due(self.active.handover_due(), Work::Handover),
            due(self.active.missed_activation_due(), Work::MissedActivation),
            due(self.active.unclassified_due(), Work::Unclassified),
            due(self.active.snapshot_due(), Work::Snapshot),
            due(self.background.next_due(&self.ctx.config.timing), Work::Background),
            due(self.background.sweep_due(), Work::Sweep),
        ]
    }

    async fn perform(&mut self, work: Work) {
        match work {
            Work::Arrival => self.dialogs.look(&mut self.ctx).await,
            Work::Handover => self.hand_over().await,
            Work::MissedActivation => {
                if let Some(window) = self.active.take_missed_activation() {
                    self.activate_if_active(window).await;
                }
            }
            Work::Unclassified => {
                if let Some(window) = self.active.take_unclassified() {
                    self.activate_if_active(window).await;
                }
            }
            Work::Snapshot => self.snapshot().await,
            Work::Background => {
                let current = self.active.window();
                self.background.read_changed(&mut self.ctx, current).await;
            }
            Work::Sweep => {
                let current = self.active.window();
                self.background.sweep_step(&mut self.ctx, current).await;
            }
        }
    }

    /// Ends the open segment and the sweep. [`next`](Self::next) yields the remaining records,
    /// then nothing.
    ///
    /// `edit` says what happens to an edit that waits to be read. A read edit ends up in the
    /// segment. The read of the edit takes a second at most, and an edit that takes longer to
    /// read is lost.
    pub async fn stop(&mut self, edit: WaitingEdit) {
        if edit == WaitingEdit::Read && self.active.has_pending_edit() {
            let limits = Limits {
                budget: self.ctx.config.limits.budget.min(STOP_READ_BUDGET),
                ..self.ctx.config.limits
            };
            let read = self.active.snapshot(&mut self.ctx, limits);
            match tokio::time::timeout(STOP_READ_TIME, read).await {
                Ok(left) => self.keep_reading(left),
                Err(_) => debug!("reading the waiting edit took too long"),
            }
        }
        self.background.stop_sweep();
        self.end_segment();
        self.phase = Phase::Stopped;
    }

    /// Gives the backend back with the next ids and the configuration, so that a new capture
    /// can follow the backend with [`resume`](Self::resume).
    pub fn into_stopped(self) -> Stopped<B> {
        Stopped {
            parts: Parts {
                ids: NextIds {
                    window: self.ctx.windows.next(),
                    segment: self.active.next_segment(),
                },
                backend: self.ctx.backend,
            },
            config: self.ctx.config,
        }
    }

    /// Takes the counts of the platform events received so far, by application name.
    pub async fn take_event_counts(&mut self) -> Vec<EventCount<String>> {
        let mut counts = Vec::new();
        for count in self.ctx.backend.take_event_counts() {
            let app = match self.ctx.app_name(&count.app).await {
                Ok(name) => name.to_owned(),
                Err(_) => format!("{:?}", count.app),
            };
            counts.push(EventCount {
                app,
                event: count.event,
                detail: count.detail,
                count: count.count,
            });
        }
        counts
    }

    async fn handle(&mut self, event: Event<B>) {
        if let Event::ScreenChanged { screen } = event {
            self.screen_changed(screen).await;
            return;
        }
        // The windows of a locked screen are hidden, and the lock screen shows the user's name.
        if self.screen == Screen::Locked {
            return;
        }
        match event {
            Event::Activated { window } => self.activate(window).await,
            Event::Deactivated { window } => self.deactivated(window).await,
            Event::TitleChanged { window, title } => {
                if self.active.window() == Some(&window) {
                    let title = self.ctx.scrubber.title(title);
                    self.active.retitle(title, &mut self.ctx.records);
                }
            }
            Event::FocusMoved { app } => {
                if !self.active.follows_app(&app)
                    && let Some(window) = self.ctx.backend.active_window_of(&app).await
                {
                    self.activate(window).await;
                }
            }
            Event::ContentChanged { app, window, cause } => {
                if self.active.concerns(&app, window.as_ref()) {
                    self.active.poke(cause.into());
                } else if let Some(window) = window {
                    self.background.note_change(&mut self.ctx, window, cause.into()).await;
                }
            }
            Event::Opened { window } => {
                let settle = self.ctx.config.timing.dialog_settle;
                self.dialogs.expect_window(window, Announcement::Opened, settle);
            }
            Event::Joined { app } => {
                self.ctx.forget_app(&app);
                self.dialogs.expect_app(app, self.ctx.config.timing.dialog_settle);
            }
            Event::Notified { notice } => notifications::notify(&mut self.ctx, notice).await,
            Event::ScreenChanged { .. } => {}
        }
    }

    /// Ends the segment and the sweep when the screen locks, and follows the desktop again when
    /// the screen unlocks.
    ///
    /// The edit that waits in the active window is dropped at the lock, because a read of a
    /// locked screen finds nothing. The windows can change while the screen is locked, so every
    /// window is read again after the unlock.
    async fn screen_changed(&mut self, screen: Screen) {
        self.announced = Some(screen);
        if std::mem::replace(&mut self.screen, screen) == screen {
            return;
        }
        match screen {
            Screen::Locked => {
                debug!("the screen locked, so capture pauses");
                self.background.stop_sweep();
                self.end_segment();
            }
            Screen::Unlocked => {
                debug!("the screen unlocked, so capture reads every window again");
                self.follow_desktop().await;
            }
        }
    }

    async fn activate(&mut self, window: B::Window) {
        let window = self.ctx.backend.top_level(window).await;
        if self.active.window() == Some(&window) {
            return;
        }
        if self.active.declines(&window) {
            debug!("the window was not captured a moment ago");
            return;
        }
        let app = self.ctx.backend.app_of(&window);
        let settle = self.ctx.config.timing.dialog_settle;
        // Capture classifies only the windows that it may read, because a classification asks
        // the application, which may hang. `None` stands for a class that is unknown.
        let class = if self.ctx.allows_app(&app).await {
            let class = match self.ctx.backend.window_class(&window).await {
                Ok(class) => Some(class),
                Err(error) => {
                    debug!(?app, %error, "the class of the activated window is unknown");
                    None
                }
            };
            self.dialogs.expect_window(window.clone(), Announcement::Activated(class), settle);
            class
        } else {
            Some(WindowClass::Regular)
        };
        if self.active.follows_app(&app) && class == Some(WindowClass::Popup) {
            debug!("staying with the window that opened a popup");
            return;
        }
        // Read an edit that waits before the segment ends, because the window may close and
        // take the typed text with it.
        if self.active.has_pending_edit() {
            self.snapshot().await;
        }
        self.end_segment();
        let Some(app_name) = self.ctx.allowed_app_name(&app).await.map(str::to_owned) else {
            return;
        };
        // `Dialogs` hands out a notification window through the `expect_window` call above, so
        // capture doesn't follow the window. A window whose class is unknown may be a
        // notification window too.
        match class {
            Some(WindowClass::Notification | WindowClass::NoticeWindow) => return,
            Some(WindowClass::Regular | WindowClass::Popup | WindowClass::Dialog) => {}
            None => {
                self.active.await_class(window, settle);
                return;
            }
        }
        let title = self.ctx.title(&window).await;
        let window_id = self.ctx.windows.id(&window);
        self.background.forget(&window);
        self.active.follow(Followed {
            window,
            window_id,
            app,
            app_name,
            title,
        });
    }

    /// Activates `window` again, when its application still has the window active.
    async fn activate_if_active(&mut self, window: B::Window) {
        let app = self.ctx.backend.app_of(&window);
        let Some(active) = self.ctx.backend.active_window_of(&app).await else {
            return;
        };
        if self.ctx.backend.top_level(active).await == window {
            self.activate(window).await;
        }
    }

    async fn deactivated(&mut self, window: B::Window) {
        let Some(app) = self.active.app() else {
            return;
        };
        // The event can come from an inner object of the current window. Then the segment ends
        // only when the application has no active window left.
        if self.active.window() != Some(&window) && *app != self.ctx.backend.app_of(&window) {
            return;
        }
        // A popup of the current window takes the active state while it is open, and the
        // segment goes on.
        let app = app.clone();
        match self.ctx.backend.active_window_of(&app).await {
            // A popup that opens or closes leaves the application without an active window for
            // a moment, so the application is asked again later. Waiting here would hold up
            // every other event, such as a dialog that appears.
            None => self.active.await_handover(),
            Some(active) => self.conclude_deactivation(Some(active)).await,
        }
    }

    /// Asks the application that lost its active window a moment ago for its active window again.
    async fn hand_over(&mut self) {
        self.active.clear_handover();
        let Some(app) = self.active.app().cloned() else {
            return;
        };
        let active = self.ctx.backend.active_window_of(&app).await;
        self.conclude_deactivation(active).await;
    }

    /// Ends the current segment when the application that lost its active window has no active
    /// window left, or when the application's active window is another window that isn't a popup
    /// of the current window.
    async fn conclude_deactivation(&mut self, active: Option<B::Window>) {
        let ended = match (active, self.active.window()) {
            (Some(active), Some(current)) if active != *current => {
                !matches!(self.ctx.backend.window_class(&active).await, Ok(WindowClass::Popup))
            }
            (Some(_), _) => false,
            (None, _) => true,
        };
        if ended {
            self.end_segment();
        }
    }

    async fn snapshot(&mut self) {
        let limits = self.ctx.config.limits;
        let left = self.active.snapshot(&mut self.ctx, limits).await;
        self.keep_reading(left);
    }

    fn end_segment(&mut self) {
        let left = self.active.end_segment(&mut self.ctx.records);
        self.keep_reading(left);
    }

    /// Goes on reading a window that left the foreground, in the background.
    fn keep_reading(&mut self, left: Option<Left<B::Window>>) {
        if let Some(left) = left {
            self.background.follow(left);
        }
    }
}

/// The first work of `schedule` that is due at `now`.
fn first_due<const N: usize>(schedule: [Option<Due>; N], now: Instant) -> Option<Work> {
    schedule.into_iter().flatten().find(|due| due.at <= now).map(|due| due.work)
}

async fn sleep_until(due: Option<Instant>) {
    match due {
        Some(due) => tokio::time::sleep_until(due).await,
        None => std::future::pending().await,
    }
}
