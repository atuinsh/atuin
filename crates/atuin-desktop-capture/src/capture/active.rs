//! Follows the active window: its segments, and the snapshots of its content.

use std::collections::VecDeque;
use std::time::{Duration, SystemTime};

use tokio::time::Instant;
use tracing::debug;

use super::background::Left;
use super::config::Timing;
use super::context::{Context, Timed};
use super::debounce::Debounce;
use super::pacing::{EmptyRetries, Pacing, Retries, Retry, SlowRetries};
use super::record::{
    Change, Record, Segment, SegmentId, Snapshot, SnapshotContent, Trigger, WindowId,
};
use super::scrubber::{Found, Seen};
use super::title::same_document;
use crate::backend::{Backend, Completeness, Error, Limits, Loading, Reading};
use crate::render::is_blank;

/// How long the active state takes to pass between a window and its popup.
const HANDOVER_TIME: Duration = Duration::from_millis(200);

/// How many times in a row capture classifies an active window again when the window's class
/// stays unknown.
const MAX_CLASS_RETRIES: u8 = 3;

/// The window that capture follows, and when its next snapshot is due.
pub struct Active<B: Backend> {
    debounce: Debounce,
    current: Option<Current<B>>,
    /// The window that capture stopped following last because the window's content isn't
    /// captured.
    declined: Option<Declined<B::Window>>,
    /// The window that became active last while its class was unknown.
    unclassified: Option<Unclassified<B::Window>>,
    /// When to ask again whether the application of the current window, which lost its active
    /// window, has an active window again.
    handover: Option<Instant>,
    /// The id of the next segment.
    next_segment: u64,
}

/// The window that capture follows.
struct Current<B: Backend> {
    window: B::Window,
    window_id: WindowId,
    app: B::App,
    app_name: String,
    title: String,
    url: Option<String>,
    started: SystemTime,
    empty: EmptyRetries,
    slow: SlowRetries,
    /// When the window's document was first found loading, while capture waits for it.
    loading_since: Option<Instant>,
    /// A window gets a segment only after it stays active long enough to be snapshotted, so
    /// the titles that a window shows while it loads never leave capture.
    segment: Option<Open>,
}

/// Whether the active window could be read for a snapshot.
enum Ready {
    Read(Timed<Reading>),
    /// The window is not ready yet, and the snapshot is due again later.
    Wait,
    /// The window's content is not captured, so capture stops following the window. The read
    /// that found it took `read_time`.
    Leave {
        read_time: Duration,
    },
}

/// A window whose content isn't captured, which capture doesn't read again for a while.
///
/// Every focus change inside such a window would otherwise make capture read the whole window
/// again to find the same kind.
struct Declined<W> {
    window: W,
    until: Instant,
    /// Whether the window was activated while it rested.
    ///
    /// The window's kind can change, such as when the user hides the terminal panel of an editor,
    /// so capture looks at the window again once the rest ends.
    missed_activation: bool,
}

/// An active window whose class was unknown at its activation.
///
/// Capture doesn't follow such a window, because the window may be a notification window,
/// which capture hands out only as a notification. Capture classifies the window again once
/// the window settles.
struct Unclassified<W> {
    window: W,
    /// When capture activates the window again, or `None` when no activation is due.
    due: Option<Instant>,
    retries: Retries<MAX_CLASS_RETRIES>,
}

/// A segment that has produced its `SegmentStarted` record.
struct Open {
    id: SegmentId,
    /// The content of the segment's last snapshot.
    last: Option<Seen>,
    /// When the segment last handed out its content in full.
    sent_in_full: Instant,
}

/// A window that capture starts to follow.
pub struct Followed<B: Backend> {
    pub window: B::Window,
    pub window_id: WindowId,
    pub app: B::App,
    pub app_name: String,
    pub title: String,
}

impl<B: Backend> Current<B> {
    /// The window that capture starts to follow, without a segment yet.
    fn new(followed: Followed<B>) -> Self {
        let Followed {
            window,
            window_id,
            app,
            app_name,
            title,
        } = followed;
        Self {
            window,
            window_id,
            app,
            app_name,
            title,
            url: None,
            started: SystemTime::now(),
            empty: EmptyRetries::default(),
            slow: SlowRetries::default(),
            loading_since: None,
            segment: None,
        }
    }

    /// Ends the open segment, and makes the window start over under `title`.
    fn start_over(&mut self, title: String, records: &mut VecDeque<Record>) {
        if let Some(open) = self.segment.take() {
            let now = SystemTime::now();
            records.push_back(Record::SegmentEnded {
                segment: open.id,
                at: now,
            });
            self.started = now;
        }
        self.title = title;
        self.url = None;
        self.empty.reset();
        self.loading_since = None;
    }

    /// Whether `url` names another document than the one that the window's segment is about.
    fn leaves_document(&self, url: Option<&str>) -> bool {
        matches!((self.url.as_deref(), url), (Some(known), Some(url)) if known != url)
    }

    /// The window's segment, which the call starts when the window has none yet.
    fn open_segment(
        &mut self,
        next_segment: &mut u64,
        records: &mut VecDeque<Record>,
    ) -> &mut Open {
        self.segment.get_or_insert_with(|| {
            let id = SegmentId(*next_segment);
            *next_segment += 1;
            records.push_back(Record::SegmentStarted(Segment {
                id,
                window: self.window_id,
                app: self.app_name.clone(),
                title: self.title.clone(),
                url: self.url.clone(),
                started: self.started,
            }));
            Open {
                id,
                last: None,
                sent_in_full: Instant::now(),
            }
        })
    }
}

impl<B: Backend> Active<B> {
    pub const fn new(timing: Timing, next_segment: u64) -> Self {
        Self {
            debounce: Debounce::new(timing),
            current: None,
            declined: None,
            unclassified: None,
            handover: None,
            next_segment,
        }
    }

    pub fn window(&self) -> Option<&B::Window> {
        self.current.as_ref().map(|current| &current.window)
    }

    /// The application of the window that capture follows.
    pub fn app(&self) -> Option<&B::App> {
        self.current.as_ref().map(|current| &current.app)
    }

    /// Whether capture follows a window of `app`.
    pub fn follows_app(&self, app: &B::App) -> bool {
        self.app() == Some(app)
    }

    /// Whether a change in `window` of `app` concerns the window that capture follows. A
    /// change without a window concerns every window of its application.
    pub fn concerns(&self, app: &B::App, window: Option<&B::Window>) -> bool {
        self.current.as_ref().is_some_and(|current| {
            window.map_or(current.app == *app, |window| current.window == *window)
        })
    }

    /// Whether capture skips an activation of `window`, because the window's content wasn't
    /// captured a moment ago.
    ///
    /// A skipped activation is due again once the window has rested. An activation of another
    /// window cancels the skipped activation.
    pub fn declines(&mut self, window: &B::Window) -> bool {
        let Some(declined) = &mut self.declined else {
            return false;
        };
        if declined.window != *window {
            declined.missed_activation = false;
            return false;
        }
        if Instant::now() >= declined.until {
            return false;
        }
        declined.missed_activation = true;
        true
    }

    /// When the activation that [`declines`](Self::declines) skipped is due again.
    pub fn missed_activation_due(&self) -> Option<Instant> {
        let declined = self.declined.as_ref().filter(|declined| declined.missed_activation)?;
        Some(declined.until)
    }

    /// Takes the window whose activation [`declines`](Self::declines) skipped.
    pub fn take_missed_activation(&mut self) -> Option<B::Window> {
        let declined = self.declined.as_mut().filter(|declined| declined.missed_activation)?;
        declined.missed_activation = false;
        Some(declined.window.clone())
    }

    /// Activates `window` again after `settle`, because the window's class was unknown at its
    /// activation. Capture gives up on the window once the retries in a row are used up.
    pub fn await_class(&mut self, window: B::Window, settle: Duration) {
        let unclassified = match &mut self.unclassified {
            Some(unclassified) if unclassified.window == window => unclassified,
            slot => slot.insert(Unclassified {
                window,
                due: None,
                retries: Retries::default(),
            }),
        };
        match unclassified.retries.try_again() {
            Retry::Again => unclassified.due = Some(Instant::now() + settle),
            Retry::Stop => {
                debug!("the class of the active window stays unknown, and capture gives up");
                // A later activation of the window starts the retries over.
                self.unclassified = None;
            }
        }
    }

    /// When the activation that waits for [`await_class`](Self::await_class) is due.
    pub fn unclassified_due(&self) -> Option<Instant> {
        self.unclassified.as_ref()?.due
    }

    /// Takes the window whose activation waits for [`await_class`](Self::await_class).
    pub fn take_unclassified(&mut self) -> Option<B::Window> {
        let unclassified = self.unclassified.as_mut()?;
        unclassified.due.take()?;
        Some(unclassified.window.clone())
    }

    /// The id of the next segment.
    pub const fn next_segment(&self) -> u64 {
        self.next_segment
    }

    /// When the next snapshot is due.
    pub fn snapshot_due(&self) -> Option<Instant> {
        self.debounce.due()
    }

    /// When to ask again whether the current application has an active window.
    pub const fn handover_due(&self) -> Option<Instant> {
        self.handover
    }

    /// Whether an edit waits to be read.
    pub fn has_pending_edit(&self) -> bool {
        self.debounce.has_pending_edit()
    }

    /// Notes a change of the kind of `trigger` in the current window.
    pub fn poke(&mut self, trigger: Trigger) {
        self.debounce.poke(Instant::now(), trigger);
    }

    /// Follows a window, whose activation is the trigger of its first snapshot.
    pub fn follow(&mut self, followed: Followed<B>) {
        self.handover = None;
        self.declined = None;
        self.unclassified = None;
        self.current = Some(Current::new(followed));
        self.poke(Trigger::Activated);
    }

    /// Takes a new title of the current window. A title of another document starts a new
    /// segment.
    pub fn retitle(&mut self, title: String, records: &mut VecDeque<Record>) {
        let Some(current) =
            self.current.as_mut().filter(|current| !same_document(&current.title, &title))
        else {
            return;
        };
        current.start_over(title, records);
        self.poke(Trigger::TitleChanged);
    }

    /// Asks the current application again later whether it has an active window.
    pub fn await_handover(&mut self) {
        self.handover = Some(Instant::now() + HANDOVER_TIME);
    }

    pub const fn clear_handover(&mut self) {
        self.handover = None;
    }

    /// Ends the segment of the current window, and stops following the window.
    pub fn end_segment(&mut self, records: &mut VecDeque<Record>) -> Option<Left<B::Window>> {
        self.debounce.reset();
        let current = self.current.take()?;
        let open = current.segment?;
        records.push_back(Record::SegmentEnded {
            segment: open.id,
            at: SystemTime::now(),
        });
        Some(Left {
            window: current.window,
            seen: open.last,
        })
    }

    /// Takes the snapshot that is due, when one is, with a read within `limits`. Returns the
    /// window when the snapshot found that capture should no longer follow it.
    pub async fn snapshot(
        &mut self,
        ctx: &mut Context<B>,
        limits: Limits,
    ) -> Option<Left<B::Window>> {
        let trigger = self.debounce.take(Instant::now())?;
        let window = self.current.as_ref()?.window.clone();
        let read = match self.read_when_ready(ctx, &window, trigger, limits).await {
            Ready::Read(read) => read,
            Ready::Wait => return None,
            Ready::Leave { read_time } => {
                let rest = Pacing::active(&ctx.config.timing).rest_after(read_time);
                let left = self.end_segment(&mut ctx.records);
                self.declined = Some(Declined {
                    window,
                    until: Instant::now() + rest,
                    missed_activation: false,
                });
                return left;
            }
        };
        if self.current.as_ref()?.title.is_empty() {
            // A window that opened a moment ago may not have had its title at activation.
            let title = ctx.title(&window).await;
            self.current.as_mut()?.title = title;
        }
        self.record(ctx, trigger, read).await;
        None
    }

    /// Reads `window` for a snapshot, unless the window is not ready to be read.
    async fn read_when_ready(
        &mut self,
        ctx: &Context<B>,
        window: &B::Window,
        trigger: Trigger,
        limits: Limits,
    ) -> Ready {
        let started = Instant::now();
        let read = ctx.read_within(window, limits).await;
        let reading = match read.value {
            Ok(reading) => reading,
            Err(error) => {
                self.retry_after_failure(trigger, &error);
                return Ready::Wait;
            }
        };
        self.debounce.paced(read.elapsed);
        if !ctx.config.captures(reading.kind) {
            // The window leaves capture without a segment, or with its segment ended, until
            // it is activated again once it has rested.
            let app = self.current.as_ref().map(|current| current.app_name.as_str());
            debug!(app, kind = ?reading.kind, "the active window is not captured");
            return Ready::Leave {
                read_time: read.elapsed,
            };
        }
        if reading.loading == Loading::InProgress && self.waits_for_loading(started, trigger) {
            return Ready::Wait;
        }
        Ready::Read(Timed {
            value: reading,
            elapsed: read.elapsed,
        })
    }

    /// Hands out a read of the current window as a snapshot of its segment, which the call
    /// starts when the window has none yet.
    async fn record(&mut self, ctx: &mut Context<B>, trigger: Trigger, read: Timed<Reading>) {
        let Some(current) = self.current.as_mut() else {
            return;
        };
        let Timed {
            value: reading,
            elapsed: read_time,
        } = read;
        current.loading_since = None;
        let blank = is_blank(&reading.root);
        if blank && current.empty.try_again() == Retry::Again {
            // An application can expose its tree a moment before the text in it. The window
            // doesn't get a segment yet, so the title that it shows until then never leaves
            // capture.
            debug!(nodes = reading.stats.nodes, "the active window has nothing readable yet");
            self.debounce.retry(Instant::now());
            return;
        }
        let url = reading.url.map(|url| ctx.scrubber.url(url));
        if current.leaves_document(url.as_deref()) {
            // The new document's title may not have arrived as an event yet.
            let title = ctx.title(&current.window).await;
            current.start_over(title, &mut ctx.records);
        }
        if url.is_some() {
            current.url = url;
        }
        let open = current.open_segment(&mut self.next_segment, &mut ctx.records);
        if blank {
            // The retries are used up, and the segment records the activity without any text.
            debug!(
                segment = open.id.0,
                nodes = reading.stats.nodes,
                "the active window has nothing readable"
            );
            return;
        }
        let completeness = reading.stats.completeness;
        let found = Found::new(reading.root, read_time, reading.stats);
        let previous = open.last;
        // A store can fail to keep a snapshot, and the snapshots that only point back to that
        // snapshot would leave the segment without content. The content goes out in full again
        // after a while.
        if open.sent_in_full.elapsed() >= ctx.config.timing.max_poll_interval {
            open.last = None;
        }
        let content = ctx.scrubber.content_since(found, &mut open.last);
        if let SnapshotContent::Changed(_) = content {
            open.sent_in_full = Instant::now();
        }
        let change = if previous.is_some_and(|seen| seen.hash() == content.hash()) {
            Change::Unchanged
        } else {
            Change::Changed
        };
        let segment = open.id;
        current.empty.reset();
        self.debounce.observed(trigger, change);
        ctx.records.push_back(Record::Snapshot(Snapshot {
            segment,
            at: SystemTime::now(),
            trigger,
            content,
        }));
        self.retry_when_out_of_time(completeness);
    }

    /// Reads the current window again soon when its read ran out of time, a few times in a row
    /// at most. The rest that the read time sets keeps a slow application from being read all
    /// the time.
    fn retry_when_out_of_time(&mut self, completeness: Completeness) {
        let Some(current) = self.current.as_mut() else {
            return;
        };
        if current.slow.after(completeness) == Retry::Again {
            debug!(
                app = current.app_name,
                "the read ran out of time, and the window is read again"
            );
            self.debounce.poke(Instant::now(), Trigger::Retried);
        }
    }

    /// Reads the current window again soon after a read failed, a few times in a row at most.
    ///
    /// An application that is busy can fail to answer once, and the snapshot of its window
    /// would otherwise wait for the next change.
    fn retry_after_failure(&mut self, trigger: Trigger, error: &Error) {
        let Some(current) = self.current.as_mut() else {
            return;
        };
        if current.slow.try_again_or_start_over() == Retry::Again {
            debug!(%error, "reading the active window failed, and the window is read again");
            self.debounce.defer(Instant::now(), trigger);
        } else {
            debug!(%error, "reading the active window failed");
        }
    }

    /// Postpones the snapshot while the document of the current window is still loading.
    /// Returns whether the snapshot waits.
    ///
    /// A page that never finishes loading is captured as it is after the longest wait.
    fn waits_for_loading(&mut self, started: Instant, trigger: Trigger) -> bool {
        let Some(current) = &mut self.current else {
            return false;
        };
        let since = *current.loading_since.get_or_insert(started);
        if started.duration_since(since) >= self.debounce.max_wait() {
            return false;
        }
        debug!("the document is still loading");
        self.debounce.defer(Instant::now(), trigger);
        true
    }
}
