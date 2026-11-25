//! The task that runs the helper when the settings enable capture.

use std::num::NonZeroU32;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use atuin_common::futures::{Backoff, Delays};
use atuin_common::rmp::frame::FrameError;
use atuin_desktop_capture::link::HelperFrame;
use futures::future;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::time::{Instant, sleep_until, timeout};
use tracing::{info, warn};

use super::DeleteError;
use super::helper::{Helper, HelperEnd, Received};
use super::helper_file::HelperFile;
use super::opened::{OpenError, Opened, Storage};
use super::process::Spawn;
use super::settings::{Change, HelperSettings};
use crate::desktop_capture::session_watch::{Desktop, SessionWatch};
use crate::desktop_capture::stop_file::{Requested, StopFile, StopFileError};
use crate::desktop_capture::store::{Deleted, Selection};
use crate::desktop_capture::{CaptureState, Published, StoreNotOpen};

/// How long a new helper has to say hello.
const HELLO_TIMEOUT: Duration = Duration::from_secs(10);

/// How long the preparation of the helper may take. On macOS, the preparation signs a copy of the
/// helper.
pub const PREPARE_TIMEOUT: Duration = Duration::from_mins(2);

/// The error of a helper that did not answer a ping in time.
pub const STOPPED_ANSWERING: &str = "the helper stopped answering";

/// How often a run ends the background segments that it has not written to for an hour.
pub const IDLE_CHECK: Duration = Duration::from_mins(5);

/// How often a helper that lacks the permission is started again.
const PERMISSION_RETRY: Duration = Duration::from_mins(1);

/// How often a helper is started again during a stretch of quick retries.
///
/// A helper that doesn't find a desktop exits at once, so a short interval costs little. The
/// quick retries let capture resume soon after a desktop session that became ready only after
/// its hint.
pub const NO_DESKTOP_RETRY: Duration = Duration::from_secs(2);

/// How long the quick retries last after the helpers stop finding a desktop session, when the
/// [`SessionWatch`] can't tell whether a helper would find one. After the quick retries, the
/// supervisor starts a helper only on a [`Hint`] of a session.
///
/// A machine that never has a desktop session, such as a server that shares the user's
/// configuration, would otherwise start a helper every 2 s for as long as the daemon runs.
pub const NO_DESKTOP_PATIENCE: Duration = Duration::from_mins(2);

/// How long the quick retries last after a [`Hint`] of a session.
///
/// Both hints come once the session is running: the autostart entry runs when the desktop session
/// is up, and the [`SessionWatch`] sees a display socket once the compositor or X server is
/// listening. The accessibility bus of a desktop session follows within seconds. An X server that
/// runs tests, such as one of `xvfb-run`, never gets a desktop, so the retries for its socket stay
/// few.
pub const HINT_PATIENCE: Duration = Duration::from_secs(20);

/// The delays before a helper that failed, or a store that failed to open, is tried again.
const BACKOFF: Backoff = Backoff::Exponential {
    initial: Duration::from_secs(1),
    max: Duration::from_mins(1),
    factor: NonZeroU32::new(2).expect("two is not zero"),
};

/// How long a helper must run for the backoff to start over.
const STEADY: Duration = Duration::from_mins(5);

/// A message from the [`Supervisor`](super::Supervisor) handle to its task.
pub enum Command {
    Settings(HelperSettings),
    /// Carries out a `start` or `stop`, and answers on `done` once the state and the stop file
    /// show it.
    Request {
        requested: Requested,
        done: oneshot::Sender<Result<(), StopFileError>>,
    },
    Delete(DeleteRequest),
    /// A desktop session started: its autostart entry ran `atuin desktop-capture login`.
    SessionStarted,
    Shutdown,
}

/// A deletion of captured content, which answers on `done`.
pub struct DeleteRequest {
    pub selection: Selection,
    pub done: oneshot::Sender<Result<Deleted, DeleteError>>,
}

/// Where the helper stands. The state shows it unless the settings or a stop take precedence.
#[derive(Debug, Clone, PartialEq, Eq)]
enum HelperPhase {
    Starting,
    /// The helper is connected and capturing.
    Running,
    NeedsPermission,
    NoDesktop,
    /// The helper is connected and doesn't read anything until the screen unlocks.
    ScreenLocked,
    /// The helper, or the store that it writes to, failed.
    Failed {
        error: String,
    },
}

impl HelperPhase {
    /// Whether `state` shows this phase.
    fn is_shown_by(&self, state: &CaptureState) -> bool {
        match (self, state) {
            (Self::Starting, CaptureState::Starting)
            | (Self::Running, CaptureState::Running)
            | (Self::NeedsPermission, CaptureState::NeedsPermission)
            | (Self::NoDesktop, CaptureState::NoDesktop)
            | (Self::ScreenLocked, CaptureState::ScreenLocked) => true,
            (Self::Failed { error }, CaptureState::Failed { error: shown }) => error == shown,
            _ => false,
        }
    }
}

impl From<&HelperPhase> for CaptureState {
    fn from(phase: &HelperPhase) -> Self {
        match phase {
            HelperPhase::Starting => Self::Starting,
            HelperPhase::Running => Self::Running,
            HelperPhase::NeedsPermission => Self::NeedsPermission,
            HelperPhase::NoDesktop => Self::NoDesktop,
            HelperPhase::ScreenLocked => Self::ScreenLocked,
            HelperPhase::Failed { error } => Self::Failed {
                error: error.clone(),
            },
        }
    }
}

/// How a run of the helper ended.
enum RunEnd {
    Helper(HelperEnd),
    /// The daemon is shutting down.
    Shutdown,
    /// The settings changed in a way that needs a new helper, or no helper.
    Respawn,
    /// A deletion is waiting for the helper to stop.
    Delete(DeleteRequest),
}

impl From<HelperEnd> for RunEnd {
    fn from(end: HelperEnd) -> Self {
        Self::Helper(end)
    }
}

/// How an attempt to open the store ended.
enum StoreOpening<'s> {
    Opened(&'s Opened),
    Failed(OpenError),
    /// The daemon is shutting down.
    Shutdown,
}

/// Whether the supervisor goes on after a wait.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Flow {
    Continue,
    Shutdown,
}

/// What a wait lasts for. Every wait ends when the settings change, and every wait for a helper
/// also ends on a `start`, which starts a helper at once.
enum Wait {
    /// Until the settings change, when the settings or the store don't need a helper.
    Change,
    /// Until the duration elapses.
    Within(Duration),
    /// Until the duration of a failure's backoff elapses, a session is reported, or the helper's
    /// file changes. A new session or a new helper is a good reason to try again at once.
    Backoff(Duration),
    /// After a helper of another build: until the helper's file changes.
    Mismatch,
    /// The wait when no desktop session is running and the [`SessionWatch`] can only guess at
    /// sessions: until a hint of a session or `until`. Without `until`, the watch looks for a
    /// session meanwhile.
    Desktop {
        until: Option<Instant>,
    },
    /// The wait when no desktop session is running and the [`SessionWatch`] knows whether a
    /// helper would find one: until the watch sees a session.
    KnownDesktop,
}

impl Wait {
    fn deadline(&self) -> Option<Instant> {
        match self {
            Self::Change | Self::Mismatch | Self::KnownDesktop => None,
            Self::Within(duration) | Self::Backoff(duration) => Some(Instant::now() + *duration),
            Self::Desktop { until } => *until,
        }
    }

    /// Whether the [`SessionWatch`] looks for a session during the wait.
    const fn watches_sessions(&self) -> bool {
        matches!(self, Self::Desktop { until: None } | Self::KnownDesktop)
    }

    /// Whether the wait ends when the helper's file changes, which can fix the failure that the
    /// wait follows.
    const fn watches_helper(&self) -> bool {
        matches!(self, Self::Backoff(_) | Self::Mismatch)
    }

    /// Whether a `start` ends the wait. Every wait for a helper ends.
    const fn ends_on_start(&self) -> bool {
        !matches!(self, Self::Change)
    }
}

/// What suggests that a desktop session may have started.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Hint {
    /// The autostart entry of a desktop session reported the session through
    /// `atuin desktop-capture login`.
    Autostart,
    /// The [`SessionWatch`] saw a session start.
    Display,
}

/// What ended one turn of a wait.
enum Woken {
    Command(Option<Command>),
    Elapsed,
    /// The [`SessionWatch`] saw a session start.
    Session,
    /// The helper's file changed.
    HelperChanged,
}

/// What woke the supervisor while a helper runs.
enum Event {
    Command(Option<Command>),
    Frame(Option<Result<HelperFrame, FrameError>>),
    /// The helper did not say hello in time.
    Silent,
    /// The time came to ping the helper, or the helper did not answer a ping in time.
    Liveness,
    /// The time came to end the idle background segments.
    IdleCheck,
}

/// The task that runs the helper when the settings enable capture.
pub struct Task {
    spawner: Arc<dyn Spawn>,
    storage: Arc<Storage>,
    settings: HelperSettings,
    requested: Requested,
    stop_file: StopFile,
    helper_phase: HelperPhase,
    /// When the current stretch of quick retries ends. `None` outside `NoDesktop`, and after a
    /// change of the settings or a request.
    quick_until: Option<Instant>,
    sessions: Box<dyn SessionWatch>,
    /// The helper's file as the last run found it. `None` before the first run.
    helper_file: Option<HelperFile>,
    /// Whether a `start` asked for the next helper, which then starts even if the
    /// [`SessionWatch`] knows that no desktop session is running.
    start_requested: bool,
    published: watch::Sender<Published>,
    commands: mpsc::UnboundedReceiver<Command>,
    delays: Delays,
}

impl Task {
    /// A task that starts with what the stop file says in `requested`. The task publishes its
    /// first state at once, and opens the store when it [runs](Self::run).
    pub fn new(
        spawner: Arc<dyn Spawn>,
        sessions: Box<dyn SessionWatch>,
        storage: Arc<Storage>,
        settings: HelperSettings,
        stop_file: StopFile,
        requested: Requested,
        commands: mpsc::UnboundedReceiver<Command>,
    ) -> Self {
        let (published, _) = watch::channel(Published {
            state: CaptureState::Starting,
            requested,
        });
        let task = Self {
            spawner,
            storage,
            settings,
            requested,
            stop_file,
            helper_phase: HelperPhase::Starting,
            quick_until: None,
            sessions,
            helper_file: None,
            start_requested: false,
            published,
            commands,
            delays: BACKOFF.delays(),
        };
        task.publish();
        task
    }

    /// A receiver of what the task publishes.
    pub fn subscribe(&self) -> watch::Receiver<Published> {
        self.published.subscribe()
    }

    /// A sender of what the task publishes, for the code that runs the task.
    pub fn publisher(&self) -> watch::Sender<Published> {
        self.published.clone()
    }

    /// Runs until the daemon shuts down or drops the capture.
    ///
    /// The store opens in the first turn of the loop, so that the daemon serves its other
    /// clients while a large store is recovering.
    pub async fn run(mut self) {
        let storage = Arc::clone(&self.storage);
        loop {
            self.publish();
            let flow = if self.needs_store().await {
                match self.open_store(&storage).await {
                    StoreOpening::Opened(opened) if self.settings.captures() => {
                        self.capture(opened).await
                    }
                    StoreOpening::Opened(_) => self.idle().await,
                    StoreOpening::Failed(err) => self.fail(err.to_string()).await,
                    StoreOpening::Shutdown => Flow::Shutdown,
                }
            } else {
                storage.leave_closed();
                self.idle().await
            };
            if flow == Flow::Shutdown {
                return;
            }
        }
    }

    /// Waits until the settings change, when the settings don't run a helper.
    ///
    /// A `start` that came before asked for a helper that these settings don't run, so it
    /// doesn't carry over to the settings that follow.
    async fn idle(&mut self) -> Flow {
        self.start_requested = false;
        self.wait(Wait::Change).await
    }

    /// The store and the index. The call opens them when they are not open yet. A store that
    /// opens clears the failure of an earlier attempt from the state.
    ///
    /// The commands that arrive during the open take effect at once, except for deletions,
    /// which run once the store is open.
    async fn open_store<'s>(&mut self, storage: &'s Storage) -> StoreOpening<'s> {
        if let Some(opened) = storage.opened() {
            return StoreOpening::Opened(opened);
        }
        let mut deletions = Vec::new();
        let opening = storage.get_or_open();
        tokio::pin!(opening);
        let opened = loop {
            tokio::select! {
                biased;
                command = self.commands.recv() => match command {
                    None | Some(Command::Shutdown) => return StoreOpening::Shutdown,
                    Some(Command::Settings(settings)) => {
                        self.take_settings(settings);
                        self.publish();
                    }
                    Some(Command::Request { requested, done }) => {
                        let result = self.request(requested).await;
                        self.start_requested |= result.is_ok() && requested == Requested::Running;
                        let _ = done.send(result);
                    }
                    Some(Command::Delete(request)) => deletions.push(request),
                    // The first helper starts once the store is open.
                    Some(Command::SessionStarted) => {}
                },
                opened = &mut opening => break opened,
            }
        };
        let opened = match opened {
            Ok(opened) => opened,
            Err(err) => {
                for request in deletions {
                    let _ = request.done.send(Err(StoreNotOpen.into()));
                }
                return StoreOpening::Failed(err);
            }
        };
        // No helper runs before the store opens, so a failure is the store's.
        if matches!(self.helper_phase, HelperPhase::Failed { .. }) {
            self.set(HelperPhase::Starting);
        }
        if !deletions.is_empty() {
            opened.close_open_segments().await;
        }
        for request in deletions {
            delete(opened, request).await;
        }
        StoreOpening::Opened(opened)
    }

    /// Whether the store should be open: the settings enable capture, or an earlier run left a
    /// store to read.
    async fn needs_store(&self) -> bool {
        self.settings.enabled || self.storage.has_store().await
    }

    /// Runs one helper, and then waits for as long as the way that the helper ended requires.
    ///
    /// If the [`SessionWatch`] knows that no desktop session is running, the task waits for a
    /// session without starting a helper, unless a `start` asked for the helper.
    async fn capture(&mut self, opened: &Opened) -> Flow {
        if !std::mem::take(&mut self.start_requested) && self.sessions.desktop() == Desktop::Absent
        {
            return self.await_desktop().await;
        }
        // A failure, or the lack of a desktop, stays in the state until the next helper
        // connects.
        if !matches!(self.helper_phase, HelperPhase::Failed { .. } | HelperPhase::NoDesktop) {
            self.set(HelperPhase::Starting);
        }
        let end = match self.run_helper(opened).await {
            RunEnd::Helper(end) => end,
            RunEnd::Shutdown => return Flow::Shutdown,
            RunEnd::Respawn => return Flow::Continue,
            // The run ended at the user's request, so the next helper starts at once.
            RunEnd::Delete(request) => {
                delete(opened, request).await;
                return Flow::Continue;
            }
        };
        match end {
            HelperEnd::Mismatch(error) => {
                warn!("{error}");
                self.set(HelperPhase::Failed { error });
                self.wait(Wait::Mismatch).await
            }
            HelperEnd::NeedsPermission => {
                self.set(HelperPhase::NeedsPermission);
                self.wait(Wait::Within(PERMISSION_RETRY)).await
            }
            HelperEnd::NoDesktop => self.await_desktop().await,
            HelperEnd::Failed(error) => self.fail(error).await,
            HelperEnd::Unanswered => self.fail(STOPPED_ANSWERING.to_owned()).await,
        }
    }

    /// Shows `error` in the state, and waits for the backoff's next delay.
    async fn fail(&mut self, error: String) -> Flow {
        warn!("desktop capture failed: {error}");
        let delay = self.delays.next_delay();
        self.set(HelperPhase::Failed { error });
        self.wait(Wait::Backoff(delay)).await
    }

    /// Shows that no desktop session is running, and waits until the next helper should start.
    ///
    /// When the [`SessionWatch`] knows whether a helper would find a desktop, the next helper
    /// starts once the watch sees one. Otherwise, the next helper starts soon during a stretch of
    /// quick retries, and after that on a hint of a session. The helper isn't at fault when a
    /// desktop session ends, so the backoff starts over.
    async fn await_desktop(&mut self) -> Flow {
        if self.helper_phase != HelperPhase::NoDesktop {
            info!("no desktop session is running, so desktop capture is waiting for one");
        }
        self.set(HelperPhase::NoDesktop);
        let wait = match self.sessions.desktop() {
            Desktop::Unknown => {
                self.delays = BACKOFF.delays();
                self.quick_retry()
            }
            Desktop::Absent => {
                self.delays = BACKOFF.delays();
                Wait::KnownDesktop
            }
            // The watch sees a desktop that the helper didn't find. For example, the user may have
            // come back as the helper was looking. The backoff keeps the helpers few if the watch
            // and the helpers keep disagreeing.
            Desktop::Present => Wait::Backoff(self.delays.next_delay()),
        };
        self.wait(wait).await
    }

    /// The next wait of the quick retries. The call starts a stretch of quick retries if none is
    /// running.
    fn quick_retry(&mut self) -> Wait {
        let quick_until = *self.quick_until.get_or_insert_with(|| {
            self.sessions.mark();
            Instant::now() + NO_DESKTOP_PATIENCE
        });
        let now = Instant::now();
        Wait::Desktop {
            until: (now < quick_until).then(|| now + NO_DESKTOP_RETRY),
        }
    }

    /// Notes that a desktop session may have started, so that the helpers that don't find a
    /// desktop start again soon for [`HINT_PATIENCE`].
    ///
    /// A hint never shortens a stretch of quick retries that is already running. A hint of the
    /// watch marks the watch again, so that the watch doesn't report the same session twice.
    fn take_hint(&mut self, hint: Hint) {
        info!(?hint, "a desktop session may have started, so desktop capture starts a helper");
        if hint == Hint::Display || self.quick_until.is_none() {
            self.sessions.mark();
        }
        let until = Instant::now() + HINT_PATIENCE;
        self.quick_until = Some(self.quick_until.map_or(until, |current| current.max(until)));
    }

    /// Runs one helper until it ends.
    async fn run_helper(&mut self, opened: &Opened) -> RunEnd {
        opened.close_open_segments().await;
        self.helper_file = Some(HelperFile::of(&self.settings.helper));
        let program = match self.prepare().await {
            Ok(program) => program,
            Err(ended) => return ended,
        };
        let process = match self.spawner.spawn(&program) {
            Ok(process) => process,
            Err(err) => {
                return HelperEnd::Failed(format!(
                    "could not start the helper {}: {err}",
                    program.display()
                ))
                .into();
            }
        };
        let mut helper = Helper::new(process);
        let stopped = self.converse(&mut helper, opened).await;
        if helper.ran_for(STEADY) {
            self.delays = BACKOFF.delays();
        }
        let ended = match stopped {
            // A helper that stopped answering would not answer `Shutdown` either, so dropping
            // `helper` below kills the helper's process.
            Some(RunEnd::Helper(HelperEnd::Unanswered)) => HelperEnd::Unanswered.into(),
            Some(ended) => {
                helper.stop(opened, self.requested).await;
                ended
            }
            None => helper.exited().await.into(),
        };
        // The run reads a background window again only when the window changes, so each
        // background segment lasts until the run stops reading.
        helper.end_background(opened).await;
        drop(helper);
        opened.close_open_segments().await;
        ended
    }

    /// Returns the program to start for the helper, and handles commands meanwhile. Returns
    /// how the run ends when a command ends it first, or when the preparation fails.
    ///
    /// Abandoning the preparation stops the programs that it runs.
    async fn prepare(&mut self) -> Result<PathBuf, RunEnd> {
        // The preparation uses its own clones of the spawner and the helper path, because
        // commands can change the settings while the preparation runs.
        let spawner = Arc::clone(&self.spawner);
        let prepared = timeout(PREPARE_TIMEOUT, spawner.prepare(self.settings.helper.clone()));
        tokio::pin!(prepared);
        loop {
            tokio::select! {
                biased;
                command = self.commands.recv() => {
                    if let Some(ended) = self.command(command, None).await {
                        return Err(ended);
                    }
                }
                prepared = &mut prepared => {
                    return match prepared {
                        Ok(Ok(program)) => Ok(program),
                        Ok(Err(err)) => Err(HelperEnd::Failed(format!(
                            "could not sign the helper: {err}"
                        ))
                        .into()),
                        Err(_) => Err(HelperEnd::Failed(format!(
                            "signing the helper took longer than {} s",
                            PREPARE_TIMEOUT.as_secs()
                        ))
                        .into()),
                    };
                }
            }
        }
    }

    /// Exchanges frames with `helper`. Returns how the run should end when the helper must be
    /// stopped, and `None` once the helper has closed its output.
    async fn converse(&mut self, helper: &mut Helper, opened: &Opened) -> Option<RunEnd> {
        let hello_by = Instant::now() + HELLO_TIMEOUT;
        let mut idle_check = Instant::now() + IDLE_CHECK;
        loop {
            let liveness = helper.liveness_due();
            let event = tokio::select! {
                biased;
                command = self.commands.recv() => Event::Command(command),
                frame = helper.frame() => Event::Frame(frame),
                () = sleep_until(hello_by), if helper.awaits_hello() => Event::Silent,
                () = sleep_until(liveness.unwrap_or(hello_by)), if liveness.is_some() => {
                    Event::Liveness
                }
                () = sleep_until(idle_check) => Event::IdleCheck,
            };
            let ended = match event {
                Event::Command(command) => {
                    let request = matches!(command, Some(Command::Request { .. }));
                    let ended = self.command(command, Some(helper)).await;
                    // The run reads a background window again only when the window changes, so
                    // each background segment lasts until the stop.
                    if request && self.requested == Requested::Stopped {
                        helper.end_background(opened).await;
                    }
                    ended
                }
                Event::Frame(Some(Ok(frame))) => self.receive(helper, frame, opened).await,
                Event::Frame(Some(Err(err))) => {
                    Some(HelperEnd::Failed(format!("could not read from the helper: {err}")).into())
                }
                Event::Frame(None) => return None,
                Event::Silent => Some(
                    HelperEnd::Failed(format!(
                        "the helper did not say hello within {} s",
                        HELLO_TIMEOUT.as_secs()
                    ))
                    .into(),
                ),
                Event::Liveness => helper.check_liveness().await.map(RunEnd::from),
                Event::IdleCheck => {
                    helper.end_idle_background(opened).await;
                    idle_check = Instant::now() + IDLE_CHECK;
                    None
                }
            };
            if ended.is_some() {
                return ended;
            }
        }
    }

    /// Handles a command during a run, before or after the helper starts. Returns how the run
    /// should end when the command ends it.
    async fn command(
        &mut self,
        command: Option<Command>,
        helper: Option<&mut Helper>,
    ) -> Option<RunEnd> {
        match command {
            None | Some(Command::Shutdown) => Some(RunEnd::Shutdown),
            Some(Command::Settings(settings)) => self.reconfigure(helper, settings).await,
            Some(Command::Request { requested, done }) => {
                let result = self.request(requested).await;
                // The start's outcome is the outcome of the helper that is starting.
                let pending = helper.as_ref().is_none_or(|helper| !helper.has_outcome());
                if result.is_ok() && requested == Requested::Running && pending {
                    self.set(HelperPhase::Starting);
                }
                let _ = done.send(result);
                // A start whose stop file could not be removed leaves capture stopped, so the
                // helper hears what took effect.
                helper?.tell(self.requested).await.map(RunEnd::from)
            }
            Some(Command::Delete(request)) => Some(RunEnd::Delete(request)),
            // A helper of the quick retries keeps running, and the hint holds for the wait after
            // it. A helper that found a desktop doesn't need a hint.
            Some(Command::SessionStarted) => {
                if self.helper_phase == HelperPhase::NoDesktop {
                    self.take_hint(Hint::Autostart);
                }
                None
            }
        }
    }

    /// Handles one frame from the helper.
    async fn receive(
        &mut self,
        helper: &mut Helper,
        frame: HelperFrame,
        opened: &Opened,
    ) -> Option<RunEnd> {
        // `reconfigure` ends the run when the settings lose their link, so this branch is only a
        // guard.
        let Some(config) = &self.settings.link else {
            return Some(RunEnd::Respawn);
        };
        match helper.receive(frame, opened, config, self.requested).await {
            Received::Nothing => {}
            Received::Connected => self.set(HelperPhase::Running),
            Received::NeedsPermission => self.set(HelperPhase::NeedsPermission),
            Received::ScreenLocked => {
                // The helper doesn't read anything while the screen is locked, so the background
                // segments end at the lock. They end before the state changes, so that a status
                // that says the screen is locked never shows them still open.
                helper.end_background(opened).await;
                self.set(HelperPhase::ScreenLocked);
            }
            Received::ScreenUnlocked => self.set(HelperPhase::Running),
            Received::Ended(ended) => return Some(ended.into()),
        }
        None
    }

    /// Applies new settings to the run. Returns [`RunEnd::Respawn`] when the settings need a new
    /// helper, or none.
    async fn reconfigure(
        &mut self,
        helper: Option<&mut Helper>,
        settings: HelperSettings,
    ) -> Option<RunEnd> {
        let change = self.settings.change_to(&settings);
        self.take_settings(settings);
        match change {
            Change::None => None,
            // Settings that turn capture off show at once, before the helper has stopped.
            Change::Respawn => {
                self.publish();
                Some(RunEnd::Respawn)
            }
            Change::Reconfigure => {
                // A helper that has not started yet is configured from the new settings when it
                // says hello.
                match (helper, &self.settings.link) {
                    (Some(helper), Some(config)) => {
                        helper.reconfigure(config).await.map(RunEnd::from)
                    }
                    _ => None,
                }
            }
        }
    }

    /// Takes on new settings.
    fn take_settings(&mut self, settings: HelperSettings) {
        self.settings = settings;
        // The settings can change after a long time without a desktop, so the next helper that
        // doesn't find a desktop is started again soon.
        self.quick_until = None;
    }

    /// Waits as `wait` says, and handles `start`, `stop`, deletions, and new settings meanwhile.
    ///
    /// A `start` that ends the wait starts the backoff over, and shows `Starting` until the next
    /// helper's outcome.
    async fn wait(&mut self, wait: Wait) -> Flow {
        let deadline = wait.deadline();
        loop {
            let command = match self.next_wake(&wait, deadline).await {
                Woken::Command(command) => command,
                Woken::Elapsed => return Flow::Continue,
                // The watch saw the desktop itself, so no quick retries follow.
                Woken::Session if matches!(wait, Wait::KnownDesktop) => {
                    info!("this user is at the screen, so desktop capture starts a helper");
                    return Flow::Continue;
                }
                Woken::Session => {
                    self.take_hint(Hint::Display);
                    return Flow::Continue;
                }
                Woken::HelperChanged => {
                    info!("the helper's file changed, so desktop capture starts the helper again");
                    self.delays = BACKOFF.delays();
                    return Flow::Continue;
                }
            };
            match command {
                None | Some(Command::Shutdown) => return Flow::Shutdown,
                Some(Command::Settings(settings)) => {
                    if settings != self.settings {
                        self.take_settings(settings);
                        return Flow::Continue;
                    }
                }
                Some(Command::Request { requested, done }) => {
                    let result = self.request(requested).await;
                    let starts =
                        result.is_ok() && requested == Requested::Running && wait.ends_on_start();
                    if starts {
                        self.delays = BACKOFF.delays();
                        self.start_requested = true;
                        self.set(HelperPhase::Starting);
                    }
                    let _ = done.send(result);
                    if starts {
                        return Flow::Continue;
                    }
                }
                Some(Command::Delete(request)) => match self.storage.opened() {
                    Some(opened) => {
                        opened.close_open_segments().await;
                        delete(opened, request).await;
                    }
                    None => {
                        let _ = request.done.send(Err(StoreNotOpen.into()));
                    }
                },
                Some(Command::SessionStarted) => match wait {
                    Wait::Desktop { .. } => {
                        self.take_hint(Hint::Autostart);
                        return Flow::Continue;
                    }
                    Wait::Backoff(_) => {
                        info!(
                            "a desktop session was reported, so desktop capture tries again at \
                             once"
                        );
                        self.delays = BACKOFF.delays();
                        return Flow::Continue;
                    }
                    // A report doesn't end the other waits. During `KnownDesktop`, the watch can
                    // already tell whether a helper would find a desktop.
                    Wait::Change | Wait::Within(_) | Wait::Mismatch | Wait::KnownDesktop => {}
                },
            }
        }
    }

    /// Waits for the next command, for `deadline` of `wait` when there is one, for the
    /// [`SessionWatch`] when `wait` watches sessions, and for the helper's file when `wait`
    /// watches the helper.
    async fn next_wake(&mut self, wait: &Wait, deadline: Option<Instant>) -> Woken {
        let watches_sessions = wait.watches_sessions();
        let sessions = &mut self.sessions;
        let watch = async {
            if watches_sessions {
                sessions.started().await;
            } else {
                future::pending::<()>().await;
            }
        };
        let helper_file = self.helper_file.as_ref().filter(|_| wait.watches_helper());
        let helper = &self.settings.helper;
        let helper_changed = async {
            match helper_file {
                Some(file) => file.changes(helper).await,
                None => future::pending().await,
            }
        };
        let elapsed = async {
            match deadline {
                Some(deadline) => sleep_until(deadline).await,
                None => future::pending().await,
            }
        };
        tokio::select! {
            command = self.commands.recv() => Woken::Command(command),
            () = elapsed => Woken::Elapsed,
            () = watch => Woken::Session,
            () = helper_changed => Woken::HelperChanged,
        }
    }

    /// Takes on `requested`, and keeps it in the stop file. The caller tells the helper.
    ///
    /// A stop takes effect before the stop file is written, so that capture stops even when the
    /// file cannot be written. A start takes effect only once the stop file is removed, so that
    /// capture never runs while a restart of the daemon would keep it stopped.
    async fn request(&mut self, requested: Requested) -> Result<(), StopFileError> {
        // A request can come after a long time without a desktop, so the next helper that doesn't
        // find a desktop is started again soon.
        self.quick_until = None;
        if requested == Requested::Running {
            self.stop_file.set(requested).await?;
        }
        self.requested = requested;
        self.publish();
        if requested == Requested::Stopped {
            self.stop_file.set(requested).await?;
        }
        Ok(())
    }

    fn set(&mut self, phase: HelperPhase) {
        if phase != HelperPhase::NoDesktop {
            self.quick_until = None;
        }
        self.helper_phase = phase;
        self.publish();
    }

    /// Publishes the state, and whether the user stopped capture, which holds in every state.
    ///
    /// The state is what the settings and the stop file say, or else the helper's phase.
    fn publish(&self) {
        let fixed = if !self.settings.enabled {
            Some(CaptureState::Disabled)
        } else if self.settings.link.is_none() {
            Some(CaptureState::NoApps)
        } else if self.requested == Requested::Stopped {
            Some(CaptureState::Stopped)
        } else {
            None
        };
        self.published.send_if_modified(|current| {
            let state_shown = match &fixed {
                Some(state) => current.state == *state,
                None => self.helper_phase.is_shown_by(&current.state),
            };
            if state_shown && current.requested == self.requested {
                return false;
            }
            current.requested = self.requested;
            if !state_shown {
                current.state = fixed.unwrap_or_else(|| CaptureState::from(&self.helper_phase));
            }
            true
        });
    }
}

/// Carries out `request`, and answers on its `done`.
///
/// The task runs one deletion at a time, and runs each to its end even when the caller stops
/// waiting, as [`Opened::delete`] requires.
async fn delete(opened: &Opened, request: DeleteRequest) {
    let deleted = opened.delete(request.selection).await.map_err(DeleteError::from);
    let _ = request.done.send(deleted);
}
