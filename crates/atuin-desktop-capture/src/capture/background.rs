//! Reads the windows in the background: every window once when capture starts, and each one
//! again when it changes.

use std::collections::{HashMap, VecDeque};
use std::hash::Hash;
use std::time::{Duration, SystemTime};

use atuin_common::collections::ForgetfulMap;
use tokio::time::Instant;
use tracing::debug;

use super::config::{BackgroundReads, Timing};
use super::context::{Context, MAX_KNOWN_WINDOWS};
use super::pacing::{Burst, Pacing, Retry, SlowRetries};
use super::record::{Background, Record, SnapshotContent, Trigger};
use super::scrubber::{Found, Seen};
use crate::backend::{Backend, Error, WindowClass};
use crate::render::is_blank;

/// A window that left the foreground, with the content that capture last handed out for it.
pub struct Left<W> {
    pub window: W,
    pub seen: Option<Seen>,
}

/// What capture knows of a window while the window is in the background.
struct Behind {
    /// The content that capture last handed out for the window.
    seen: Option<Seen>,
    /// The earliest time when changes can make capture read the window again.
    rests_until: Instant,
    slow: SlowRetries,
}

impl Behind {
    fn new(seen: Option<Seen>) -> Self {
        Self {
            seen,
            rests_until: Instant::now(),
            slow: SlowRetries::default(),
        }
    }
}

/// The progress of the sweep of every window.
enum Sweep<W> {
    /// The sweep waits for the applications that are still joining.
    Waiting {
        due: Instant,
    },
    /// The windows wait to be read, one at a time.
    Reading {
        windows: VecDeque<W>,
        due: Instant,
    },
    Done,
}

/// The windows in the background that capture read or follows, and the sweep.
///
/// Nothing is followed or swept when background reads are off.
pub struct BackgroundWindows<B: Backend> {
    reads: BackgroundReads,
    /// Handles of closed windows are never reported as gone, so the windows are forgotten now
    /// and then to bound the memory.
    behind: ForgetfulMap<B::Window, Behind>,
    /// The changes that wait to be read, by window. Each change leaves the map when capture
    /// reads its window, so the map holds only the windows that capture schedules.
    changes: HashMap<B::Window, Burst>,
    sweep: Sweep<B::Window>,
}

impl<B: Backend> BackgroundWindows<B> {
    pub fn new(reads: BackgroundReads) -> Self {
        Self {
            reads,
            behind: ForgetfulMap::new(MAX_KNOWN_WINDOWS),
            changes: HashMap::new(),
            sweep: Sweep::Done,
        }
    }

    /// Wakes the applications that need it, and starts the sweep of every window once the
    /// applications have had time to join.
    pub async fn start_sweep(&mut self, ctx: &Context<B>) {
        if self.reads == BackgroundReads::Off {
            return;
        }
        // Applications that join once a client connects need a moment.
        let joining = ctx.backend.wake_apps().await.unwrap_or_default();
        self.sweep = Sweep::Waiting {
            due: Instant::now() + joining,
        };
    }

    pub fn stop_sweep(&mut self) {
        self.sweep = Sweep::Done;
    }

    /// When the sweep next lists or reads a window.
    pub const fn sweep_due(&self) -> Option<Instant> {
        match &self.sweep {
            Sweep::Waiting { due } | Sweep::Reading { due, .. } => Some(*due),
            Sweep::Done => None,
        }
    }

    /// Follows a window that left the foreground from its last content.
    pub fn follow(&mut self, left: Left<B::Window>) {
        if self.reads == BackgroundReads::On {
            self.behind.insert(left.window, Behind::new(left.seen));
        }
    }

    /// Stops following `window`, which came to the foreground.
    pub fn forget(&mut self, window: &B::Window) {
        self.behind.remove(window);
        self.changes.remove(window);
    }

    /// Notes that the content of `window`, which is in the background, changed.
    pub async fn note_change(&mut self, ctx: &mut Context<B>, window: B::Window, trigger: Trigger) {
        if self.reads == BackgroundReads::Off {
            return;
        }
        let now = Instant::now();
        if let Some(burst) = self.changes.get_mut(&window) {
            *burst = burst.with_change(now);
            return;
        }
        if !self.behind.contains_key(&window) && !ctx.allows_app(&ctx.backend.app_of(&window)).await
        {
            return;
        }
        self.changes.insert(window, Burst::new(now, trigger));
    }

    /// When the changes of a window are due to be read first.
    pub fn next_due(&self, timing: &Timing) -> Option<Instant> {
        self.changes.iter().map(|(window, burst)| self.due(window, burst, timing)).min()
    }

    /// Reads the background window whose changes are due first.
    pub async fn read_changed(&mut self, ctx: &mut Context<B>, current: Option<&B::Window>) {
        let timing = &ctx.config.timing;
        let Some(window) = self
            .changes
            .iter()
            .min_by_key(|&(window, burst)| self.due(window, burst, timing))
            .map(|(window, _)| window.clone())
        else {
            return;
        };
        let Some(burst) = self.changes.remove(&window) else {
            return;
        };
        self.read(ctx, window, burst.trigger(), current).await;
    }

    /// When the changes of `burst` in `window` are due to be read, once they have settled and
    /// the window has rested.
    fn due(&self, window: &B::Window, burst: &Burst, timing: &Timing) -> Instant {
        let settled = burst.settled(timing);
        self.behind.get(window).map_or(settled, |behind| settled.max(behind.rests_until))
    }

    /// Lists the windows to sweep, or reads the next one of them.
    pub async fn sweep_step(&mut self, ctx: &mut Context<B>, current: Option<&B::Window>) {
        match &mut self.sweep {
            Sweep::Done => {}
            Sweep::Waiting { .. } => {
                let windows = sweep_list(ctx).await;
                debug!(windows = windows.len(), "sweeping every window");
                self.sweep = Sweep::Reading {
                    windows,
                    due: Instant::now(),
                };
            }
            Sweep::Reading { windows, .. } => {
                let Some(window) = windows.pop_front() else {
                    self.sweep = Sweep::Done;
                    return;
                };
                let read_time = self.read(ctx, window, Trigger::Swept, current).await;
                let rest = Pacing::sweep(&ctx.config.timing).rest_after(read_time);
                if let Sweep::Reading { due, .. } = &mut self.sweep {
                    *due = Instant::now() + rest;
                }
            }
        }
    }

    /// Reads `window` in the background, and hands out its content when the content differs
    /// from what capture last saw of the window. Returns how long the read took.
    ///
    /// The active window, popups, terminals, windows that the configuration doesn't capture,
    /// and windows without content are left out. Notification windows are left out because
    /// `Dialogs` hands out each one once as a [`Record::Notification`]. A window whose class is
    /// unknown may be a notification window, so it is left out until its next change.
    async fn read(
        &mut self,
        ctx: &mut Context<B>,
        window: B::Window,
        trigger: Trigger,
        current: Option<&B::Window>,
    ) -> Duration {
        if current == Some(&window) {
            return Duration::ZERO;
        }
        let app = ctx.backend.app_of(&window);
        if !ctx.allows_app(&app).await {
            return Duration::ZERO;
        }
        match ctx.backend.window_class(&window).await {
            Ok(WindowClass::Regular | WindowClass::Dialog) => {}
            Ok(WindowClass::Popup | WindowClass::Notification | WindowClass::NoticeWindow) => {
                return Duration::ZERO;
            }
            Err(error) => {
                debug!(?app, %error, "the class of a background window is unknown");
                return Duration::ZERO;
            }
        }
        let read = ctx.read(&window).await;
        let rest = Pacing::background(&ctx.config.timing).rest_after(read.elapsed);
        let reading = match read.value {
            Ok(reading) => reading,
            Err(error) => {
                self.retry_after_failure(window, trigger, rest, &error);
                return read.elapsed;
            }
        };
        let now = Instant::now();
        let behind = self.behind.get_or_insert_with(window.clone(), || Behind::new(None));
        behind.rests_until = now + rest;
        if behind.slow.after(reading.stats.completeness) == Retry::Again {
            debug!(?app, "the read ran out of time, and the window is read again");
            retry(&mut self.changes, window.clone(), Burst::new(now, Trigger::Retried));
        }
        if !ctx.config.captures(reading.kind) || is_blank(&reading.root) {
            return read.elapsed;
        }
        let found = Found::new(reading.root, read.elapsed, reading.stats);
        let SnapshotContent::Changed(content) = ctx.scrubber.content_since(found, &mut behind.seen)
        else {
            debug!(?app, "the background window is unchanged");
            return read.elapsed;
        };
        let Some(app_name) = ctx.allowed_app_name(&app).await.map(str::to_owned) else {
            return read.elapsed;
        };
        let title = ctx.title(&window).await;
        let window = ctx.windows.id(&window);
        ctx.records.push_back(Record::Background(Background {
            window,
            app: app_name,
            title,
            url: reading.url.map(|url| ctx.scrubber.url(url)),
            at: SystemTime::now(),
            trigger,
            content,
        }));
        read.elapsed
    }

    /// Reads `window` again once it has rested for `rest` after its read for `trigger` failed, a
    /// few times in a row at most.
    ///
    /// An application that is busy can fail to answer once. A window whose reads keep failing
    /// waits for its next change. Capture keeps the content that it last handed out for the
    /// window, so a later read that finds the same content doesn't hand it out again.
    fn retry_after_failure(
        &mut self,
        window: B::Window,
        trigger: Trigger,
        rest: Duration,
        error: &Error,
    ) {
        let behind = self.behind.get_or_insert_with(window.clone(), || Behind::new(None));
        if behind.slow.try_again_or_start_over() == Retry::Again {
            debug!(%error, "reading a background window failed, and the window is read again");
            let now = Instant::now();
            behind.rests_until = now + rest;
            retry(&mut self.changes, window, Burst::new(now, trigger));
        } else {
            debug!(%error, "reading a background window failed");
            self.changes.remove(&window);
        }
    }
}

/// Reads `window` again once the window has rested, as `retry` describes.
///
/// A change that arrived during the read keeps its trigger and its time. The time of the retry
/// would only postpone that change's read, because nothing changed at that time.
fn retry<W: Eq + Hash>(changes: &mut HashMap<W, Burst>, window: W, retry: Burst) {
    changes.entry(window).or_insert(retry);
}

/// Every window of every allowed application, with the background tabs of each after it.
async fn sweep_list<B: Backend>(ctx: &mut Context<B>) -> VecDeque<B::Window> {
    let mut windows = VecDeque::new();
    for app in ctx.backend.apps().await.unwrap_or_default() {
        if !ctx.allows_app(&app).await {
            continue;
        }
        for window in ctx.backend.windows(&app).await.unwrap_or_default() {
            // Background tabs follow the window that holds them.
            let hidden = ctx.backend.hidden_documents(&window).await;
            windows.push_back(window);
            windows.extend(hidden);
        }
    }
    windows
}
