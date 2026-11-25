use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime};

use atuin_common::path::PathBufExt as _;
use atuin_desktop_capture::Scope;
use atuin_desktop_capture::link::{DaemonFrame, Mode};
use futures::future::{self, BoxFuture};
use rstest::rstest;
use tokio::time::Instant;

use super::helper::{SEND_TIMEOUT, SHUTDOWN_GRACE};
use super::link::{PING_INTERVAL, PONG_TIMEOUT};
use super::task::{
    HINT_PATIENCE, IDLE_CHECK, NO_DESKTOP_PATIENCE, NO_DESKTOP_RETRY, PREPARE_TIMEOUT,
    STOPPED_ANSWERING,
};
use super::*;
use crate::desktop_capture::DesktopCapture;
use crate::desktop_capture::signing::SigningError;
use crate::desktop_capture::stop_file::StopFile;
use crate::desktop_capture::store::{
    DesktopStore, Kinds, RunState, SegmentFilter, SegmentKind, SegmentSummary, StoreError, Which,
};
use crate::desktop_capture::testing::records::{everything, snapshot, started, window, write_all};
use crate::desktop_capture::testing::{
    FakeSpawner, HELPER, HOST, HeldOpen, Home, PATIENCE, Screen, Script, SessionStarts, eventually,
    home, paused_home, state_becomes, state_becomes_within, store_opened, until, unwatched,
};
use crate::grpc::versioning::BUILD_STR;

/// Opens a capture of gedit, and waits until the capture is running.
async fn running_capture(home: &Home, spawner: &Arc<FakeSpawner>) -> DesktopCapture {
    let capture = home.open(&home.settings(&["gedit"]), spawner).await;
    state_becomes(&capture, |state| *state == CaptureState::Running).await;
    capture
}

/// Waits until `capture` fails, and returns its error.
async fn failure(capture: &DesktopCapture) -> String {
    failure_within(capture, crate::desktop_capture::testing::PATIENCE).await
}

/// Waits up to `patience` until `capture` fails, and returns its error.
async fn failure_within(capture: &DesktopCapture, patience: Duration) -> String {
    let failed = |state: &CaptureState| matches!(state, CaptureState::Failed { .. });
    match state_becomes_within(capture, patience, failed).await {
        CaptureState::Failed { error } => error,
        state => unreachable!("{state:?}"),
    }
}

/// Opens a capture of gedit with `spawner`, which doesn't need a fake helper.
async fn open_with(home: &Home, spawner: Arc<dyn Spawn>) -> DesktopCapture {
    let settings = home.settings(&["gedit"]);
    let capture = DesktopCapture::open_with(&settings, HOST, spawner, unwatched()).await;
    store_opened(&capture, &settings).await;
    capture
}

/// The stop file of the captures in `home`.
fn stop_file(home: &Home) -> StopFile {
    StopFile::in_dir(home.settings(&[]).desktop_capture_dir())
}

/// Counts how many times a value that holds the counter is dropped.
struct DropCounter(Arc<AtomicUsize>);

impl Drop for DropCounter {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

/// A spawner whose preparation of the helper never ends.
#[derive(Default)]
struct HangingSpawner {
    prepares: AtomicUsize,
    /// How many preparations were abandoned.
    dropped: Arc<AtomicUsize>,
}

impl HangingSpawner {
    fn prepares(&self) -> usize {
        self.prepares.load(Ordering::SeqCst)
    }

    fn dropped(&self) -> usize {
        self.dropped.load(Ordering::SeqCst)
    }
}

impl Spawn for HangingSpawner {
    fn prepare(&self, _program: PathBuf) -> BoxFuture<'_, Result<PathBuf, SigningError>> {
        self.prepares.fetch_add(1, Ordering::SeqCst);
        let counter = DropCounter(Arc::clone(&self.dropped));
        Box::pin(async move {
            let _counter = counter;
            future::pending().await
        })
    }

    fn spawn(&self, _program: &Path) -> std::io::Result<HelperProcess> {
        unreachable!("the preparation never ends")
    }
}

/// A spawner that panics when it prepares the helper.
struct PanickingSpawner;

impl Spawn for PanickingSpawner {
    fn prepare(&self, _program: PathBuf) -> BoxFuture<'_, Result<PathBuf, SigningError>> {
        panic!("the spawner broke")
    }

    fn spawn(&self, _program: &Path) -> std::io::Result<HelperProcess> {
        unreachable!("the preparation panics")
    }
}

#[rstest]
#[tokio::test(start_paused = true)]
async fn hello_build_mismatch_fails_without_respawn(#[from(paused_home)] home: Home) {
    let spawner = FakeSpawner::new([Script::OtherBuild]);
    let capture = home.open(&home.settings(&["gedit"]), &spawner).await;

    let error = failure(&capture).await;
    tokio::time::sleep(Duration::from_secs(5)).await;

    assert_eq!(
        error,
        format!(
            "helper build 0123456, daemon build {BUILD_STR}: install the atuin-desktop-capture of \
             the daemon's build, and capture starts again by itself"
        )
    );
    assert_eq!(spawner.spawns(), 1);
    assert!(spawner.configures().is_empty());
}

#[rstest]
#[tokio::test(start_paused = true)]
async fn missing_helper_binary_fails(#[from(paused_home)] home: Home) {
    let spawner = FakeSpawner::new([Script::Missing]);
    let capture = home.open(&home.settings(&["gedit"]), &spawner).await;

    let error = failure(&capture).await;

    assert!(error.contains(HELPER), "{error}");
}

/// A failed signing shows in the state, and the next attempt follows the backoff.
#[rstest]
#[tokio::test(start_paused = true)]
async fn signing_failure_fails_and_retries(#[from(paused_home)] home: Home) {
    let spawner = FakeSpawner::new([Script::SigningFails, Script::Serve]);
    let capture = home.open(&home.settings(&["gedit"]), &spawner).await;

    let error = failure(&capture).await;
    assert_eq!(spawner.spawns(), 0);
    state_becomes(&capture, |state| *state == CaptureState::Running).await;

    assert!(error.starts_with("could not sign the helper: "), "{error}");
    assert_eq!(spawner.spawns(), 1);
}

#[rstest]
#[tokio::test(start_paused = true)]
async fn needs_permission_retries_every_minute(#[from(paused_home)] home: Home) {
    let spawner = FakeSpawner::new([Script::NoPermission]);
    let capture = home.open(&home.settings(&["gedit"]), &spawner).await;
    until(|| spawner.spawns() == 1).await;
    let first = Instant::now();

    state_becomes(&capture, |state| *state == CaptureState::NeedsPermission).await;
    tokio::time::sleep_until(first + Duration::from_secs(61)).await;

    assert_eq!(spawner.spawns(), 2);
    assert_eq!(capture.state(), CaptureState::NeedsPermission);
}

#[rstest]
#[tokio::test(start_paused = true)]
async fn crash_closes_the_segment_and_respawns(#[from(paused_home)] home: Home) {
    let spawner = FakeSpawner::new([Script::CrashAfterSnapshot, Script::Serve]);
    let capture = home.open(&home.settings(&["gedit"]), &spawner).await;

    // The state shows the crash as the backoff begins.
    let _ = failure(&capture).await;
    let crashed = Instant::now();
    until(|| spawner.spawns() == 2).await;
    // The first delay of the backoff is 1 s, with up to 10% of jitter.
    let respawned = spawner.spawn_times()[1] - crashed;
    assert!(respawned >= Duration::from_millis(900), "{respawned:?}");
    assert!(respawned <= Duration::from_millis(1100), "{respawned:?}");

    let store = capture.store().expect("the store is open");
    let segments = store.segments(everything()).await.expect("segments");
    assert_eq!(segments.len(), 1);
    assert!(segments[0].ended.is_some(), "{:?}", segments[0]);
    state_becomes(&capture, |state| *state == CaptureState::Running).await;
}

/// When the desktop session ends, the helper is started again every 2 s until the next
/// session, however far the backoff of earlier failures had grown.
#[rstest]
#[tokio::test(start_paused = true)]
async fn a_new_desktop_session_is_captured_soon(#[from(paused_home)] home: Home) {
    let crashes = [Script::CrashAfterSnapshot; 4];
    let session = [Script::DesktopEnds, Script::NoDesktop, Script::NoDesktop, Script::Serve];
    let spawner = FakeSpawner::new(crashes.into_iter().chain(session));
    let capture = home.open(&home.settings(&["gedit"]), &spawner).await;

    state_becomes(&capture, |state| *state == CaptureState::NoDesktop).await;
    let running = state_becomes(&capture, |state| *state == CaptureState::Running).await;

    assert_eq!(running, CaptureState::Running);
    let spawned = spawner.spawn_times();
    assert_eq!(spawned.len(), crashes.len() + session.len());
    // Each helper of the session starts 2 s after the one before it. The backoff after four
    // crashes would wait 16 s.
    let retries: Vec<_> =
        spawned[crashes.len()..].windows(2).map(|pair| pair[1] - pair[0]).collect();
    assert_eq!(retries, [NO_DESKTOP_RETRY; 3]);
}

/// While no desktop session is running, the state stays `NoDesktop` across the retries.
#[rstest]
#[tokio::test(start_paused = true)]
async fn no_desktop_stays_the_state_between_retries(#[from(paused_home)] home: Home) {
    let spawner = FakeSpawner::new([Script::NoDesktop]);
    let capture = home.open(&home.settings(&["gedit"]), &spawner).await;
    state_becomes(&capture, |state| *state == CaptureState::NoDesktop).await;

    let mut published = capture.watch();
    published.mark_unchanged();
    tokio::time::sleep(NO_DESKTOP_RETRY * 5).await;

    assert!(spawner.spawns() >= 5, "{}", spawner.spawns());
    assert!(!published.has_changed().expect("the supervisor is running"));
}

/// What the user does after a long wait for a desktop.
#[derive(Debug, Clone, Copy)]
enum Nudge {
    ChangeSettings,
    Start,
}

/// Once no desktop session has run for a while, no helper starts until something suggests a
/// session. A change of the settings or a request makes the helper start often again.
#[rstest]
#[tokio::test(start_paused = true)]
async fn a_long_wait_for_a_desktop_starts_no_helpers(
    #[from(paused_home)] home: Home,
    #[values(Nudge::ChangeSettings, Nudge::Start)] nudge: Nudge,
) {
    let spawner = FakeSpawner::new([Script::NoDesktop]);
    let capture = home.open(&home.settings(&["gedit"]), &spawner).await;
    state_becomes(&capture, |state| *state == CaptureState::NoDesktop).await;

    tokio::time::sleep(NO_DESKTOP_PATIENCE).await;
    let quick = spawner.spawns();
    tokio::time::sleep(Duration::from_hours(24)).await;
    let idle = spawner.spawns() - quick;
    match nudge {
        Nudge::ChangeSettings => capture.apply_settings(&home.settings(&["gedit", "kate"])),
        Nudge::Start => capture.start().await.expect("start"),
    }
    let before = spawner.spawns();
    tokio::time::sleep(NO_DESKTOP_RETRY * 6).await;
    let nudged = spawner.spawns() - before;

    // 60 helpers start in the first 2 minutes, at one every 2 s.
    assert!(quick >= 50, "{quick}");
    // The quick wait that runs as the patience ends can start one more.
    assert!(idle <= 1, "{idle}");
    assert!(nudged >= 5, "{nudged}");
    assert_eq!(capture.state(), CaptureState::NoDesktop);
}

/// What tells the supervisor that a desktop session may have started.
#[derive(Debug, Clone, Copy)]
enum Hint {
    /// The autostart entry reports the session through `atuin desktop-capture login`.
    Autostart,
    /// The session watch sees a session start.
    Watch,
}

impl Hint {
    fn give(self, capture: &DesktopCapture, starts: &SessionStarts) {
        match self {
            Self::Autostart => capture.session_started(),
            Self::Watch => starts.start(),
        }
    }
}

/// A hint of a session ends a long wait for a desktop at once.
#[rstest]
#[tokio::test(start_paused = true)]
async fn a_hint_of_a_session_ends_a_long_wait_at_once(
    #[from(paused_home)] home: Home,
    #[values(Hint::Autostart, Hint::Watch)] hint: Hint,
) {
    let starts = SessionStarts::default();
    let spawner = FakeSpawner::new([Script::NoDesktop]);
    let capture = home.open_watched(&home.settings(&["gedit"]), &spawner, starts.watch()).await;
    state_becomes(&capture, |state| *state == CaptureState::NoDesktop).await;
    tokio::time::sleep(NO_DESKTOP_PATIENCE + Duration::from_hours(24)).await;
    let before = spawner.spawns();
    spawner.script_next([Script::Serve]);

    hint.give(&capture, &starts);

    // Nothing else would start a helper, and a paused clock would run the wait out.
    state_becomes(&capture, |state| *state == CaptureState::Running).await;
    assert_eq!(spawner.spawns(), before + 1);
}

/// A session can become ready a little after its hint, so the helper starts often again for
/// 20 s after a hint.
#[rstest]
#[tokio::test(start_paused = true)]
async fn a_hint_of_a_session_brings_back_the_quick_retries(
    #[from(paused_home)] home: Home,
    #[values(Hint::Autostart, Hint::Watch)] hint: Hint,
) {
    let starts = SessionStarts::default();
    let spawner = FakeSpawner::new([Script::NoDesktop]);
    let capture = home.open_watched(&home.settings(&["gedit"]), &spawner, starts.watch()).await;
    state_becomes(&capture, |state| *state == CaptureState::NoDesktop).await;
    tokio::time::sleep(NO_DESKTOP_PATIENCE * 2).await;
    let before = spawner.spawns();

    hint.give(&capture, &starts);
    tokio::time::sleep(HINT_PATIENCE + Duration::from_hours(1)).await;

    // One helper at once, and one every 2 s for the patience.
    let hinted = spawner.spawns() - before;
    let expected =
        usize::try_from(HINT_PATIENCE.as_secs() / NO_DESKTOP_RETRY.as_secs()).expect("few");
    assert!((expected - 2..=expected + 2).contains(&hinted), "{hinted} of {expected}");
    assert_eq!(capture.state(), CaptureState::NoDesktop);
}

/// A session that is reported while the last helper of the quick retries is running still holds
/// for the wait after that helper.
#[rstest]
#[tokio::test(start_paused = true)]
async fn a_reported_session_during_the_last_helper_of_the_quick_retries_holds(
    #[from(paused_home)] home: Home,
) {
    let spawner = FakeSpawner::new([Script::NoDesktop]);
    let capture = home.open(&home.settings(&["gedit"]), &spawner).await;
    state_becomes(&capture, |state| *state == CaptureState::NoDesktop).await;
    tokio::time::sleep(NO_DESKTOP_PATIENCE - NO_DESKTOP_RETRY).await;
    spawner.script_next([Script::SlowNoDesktop, Script::NoDesktop]);
    let before = spawner.spawns();
    // The slow helper runs past the end of the quick retries.
    until(|| spawner.spawns() == before + 1).await;

    capture.session_started();
    tokio::time::sleep(NO_DESKTOP_RETRY * 10).await;

    let after = spawner.spawns() - before - 1;
    assert!(after >= 5, "{after}");
}

/// A reported session ends the backoff after a failure, and the backoff starts over.
#[rstest]
#[tokio::test(start_paused = true)]
async fn a_reported_session_ends_the_backoff_of_a_failure(#[from(paused_home)] home: Home) {
    // A missing helper fails without writing to the store, so the clock moves only with the
    // backoff.
    let failures = [Script::Missing; 4];
    let spawner = FakeSpawner::new(failures.into_iter().chain([Script::Serve]));
    let capture = home.open(&home.settings(&["gedit"]), &spawner).await;
    until(|| spawner.spawns() == failures.len()).await;
    let _ = failure(&capture).await;
    let failed = Instant::now();

    capture.session_started();

    state_becomes(&capture, |state| *state == CaptureState::Running).await;
    // The backoff after four failures would wait 8 s.
    assert!(failed.elapsed() < Duration::from_secs(1), "{:?}", failed.elapsed());
}

/// A reported session doesn't change anything when the settings don't select any application.
#[rstest]
#[tokio::test(start_paused = true)]
async fn a_reported_session_without_apps_starts_no_helper(#[from(paused_home)] home: Home) {
    let spawner = FakeSpawner::new([Script::NoDesktop]);
    let capture = home.open(&home.settings(&["gedit"]), &spawner).await;
    state_becomes(&capture, |state| *state == CaptureState::NoDesktop).await;
    capture.apply_settings(&home.settings(&[]));
    state_becomes(&capture, |state| *state == CaptureState::NoApps).await;
    let before = spawner.spawns();

    capture.session_started();
    tokio::time::sleep(Duration::from_secs(10)).await;

    assert_eq!(spawner.spawns(), before);
}

/// When a session that the watch sees starts during the quick retries.
#[derive(Debug, Clone, Copy)]
enum Started {
    /// Before the helpers stopped finding a desktop, which the watch doesn't count.
    BeforeTheWait,
    /// During the quick retries, after which the watch reports the session.
    DuringTheQuickRetries,
}

/// The watch counts a session that started during the quick retries, so that a session that
/// became ready only after them still gets a helper soon.
#[rstest]
#[case::before_the_wait(Started::BeforeTheWait, 0..=1)]
#[case::during_the_quick_retries(Started::DuringTheQuickRetries, 5..=12)]
#[tokio::test(start_paused = true)]
async fn the_watch_counts_the_sessions_since_the_wait_began(
    #[from(paused_home)] home: Home,
    #[case] started: Started,
    #[case] spawns_after_the_patience: std::ops::RangeInclusive<usize>,
) {
    let starts = SessionStarts::default();
    if matches!(started, Started::BeforeTheWait) {
        starts.start();
    }
    let spawner = FakeSpawner::new([Script::NoDesktop]);
    let capture = home.open_watched(&home.settings(&["gedit"]), &spawner, starts.watch()).await;
    state_becomes(&capture, |state| *state == CaptureState::NoDesktop).await;
    tokio::time::sleep(NO_DESKTOP_RETRY * 3).await;
    if matches!(started, Started::DuringTheQuickRetries) {
        starts.start();
    }

    tokio::time::sleep(NO_DESKTOP_PATIENCE).await;
    let quick = spawner.spawns();
    tokio::time::sleep(NO_DESKTOP_PATIENCE + Duration::from_hours(1)).await;

    let after = spawner.spawns() - quick;
    assert!(spawns_after_the_patience.contains(&after), "{after}");
}

/// Where the helper stands when a session is reported.
#[derive(Debug, Clone, Copy)]
enum Standing {
    Running,
    NeedsPermission,
}

/// A reported session doesn't start a helper while a helper is running, or while the helper
/// lacks the permission.
#[rstest]
#[tokio::test(start_paused = true)]
async fn a_reported_session_starts_no_helper_while_one_found_a_desktop(
    #[from(paused_home)] home: Home,
    #[values(Standing::Running, Standing::NeedsPermission)] standing: Standing,
) {
    let (script, state) = match standing {
        Standing::Running => (Script::Serve, CaptureState::Running),
        Standing::NeedsPermission => (Script::NoPermission, CaptureState::NeedsPermission),
    };
    let spawner = FakeSpawner::new([script]);
    let capture = home.open(&home.settings(&["gedit"]), &spawner).await;
    state_becomes(&capture, |shown| *shown == state).await;

    capture.session_started();
    tokio::time::sleep(Duration::from_secs(10)).await;

    assert_eq!(spawner.spawns(), 1);
    assert_eq!(capture.state(), state);
}

/// What a capture waits on when a test asks it to start.
#[derive(Debug, Clone, Copy)]
enum Waiting {
    /// The backoff after failures, which has grown to 16 s.
    Backoff,
    /// The minute before a helper that lacks the permission starts again.
    Permission,
    /// The wait after a helper of another build, which only a new helper file ends.
    Mismatch,
    /// The wait for a desktop session after the quick retries, when the watch can only guess.
    GuessedDesktop,
    /// The wait for a desktop session when the watch knows that the user is away.
    KnownDesktop,
}

impl Waiting {
    /// The helpers that bring a capture into the wait.
    fn scripts(self) -> Vec<Script> {
        match self {
            Self::Backoff => vec![Script::Missing; 5],
            Self::Permission => vec![Script::NoPermission],
            Self::Mismatch => vec![Script::OtherBuild],
            Self::GuessedDesktop => vec![Script::NoDesktop],
            // No helper starts while the user is away.
            Self::KnownDesktop => Vec::new(),
        }
    }

    /// Waits until the capture is in the wait, and long enough that only a start ends it soon.
    async fn reached(self, capture: &DesktopCapture, spawner: &FakeSpawner) {
        let scripts = self.scripts().len();
        until(|| spawner.spawns() == scripts).await;
        let state = match self {
            Self::Backoff | Self::Mismatch => {
                let _ = failure(capture).await;
                return;
            }
            Self::Permission => CaptureState::NeedsPermission,
            Self::GuessedDesktop | Self::KnownDesktop => CaptureState::NoDesktop,
        };
        state_becomes(capture, |shown| *shown == state).await;
        if matches!(self, Self::GuessedDesktop) {
            tokio::time::sleep(NO_DESKTOP_PATIENCE + Duration::from_hours(1)).await;
        }
    }
}

/// A start ends every wait for a helper, and starts a helper at once.
#[rstest]
#[tokio::test(start_paused = true)]
async fn a_start_ends_every_wait_for_a_helper(
    #[from(paused_home)] home: Home,
    #[values(
        Waiting::Backoff,
        Waiting::Permission,
        Waiting::Mismatch,
        Waiting::GuessedDesktop,
        Waiting::KnownDesktop
    )]
    waiting: Waiting,
) {
    let screen = Screen::new(false);
    let watch = match waiting {
        Waiting::KnownDesktop => screen.watch(),
        _ => unwatched(),
    };
    let spawner = FakeSpawner::new(waiting.scripts());
    let capture = home.open_watched(&home.settings(&["gedit"]), &spawner, watch).await;
    waiting.reached(&capture, &spawner).await;
    let before = spawner.spawns();
    spawner.script_next([Script::Serve]);
    let asked = Instant::now();

    capture.start().await.expect("start");

    state_becomes(&capture, |state| *state == CaptureState::Running).await;
    assert_eq!(spawner.spawns(), before + 1);
    assert!(asked.elapsed() < Duration::from_millis(100), "{:?}", asked.elapsed());
}

/// A start ends the backoff, and the backoff after it starts over at its first delay.
#[rstest]
#[tokio::test(start_paused = true)]
async fn a_start_starts_the_backoff_over(#[from(paused_home)] home: Home) {
    let spawner = FakeSpawner::new([Script::Missing]);
    let capture = home.open(&home.settings(&["gedit"]), &spawner).await;
    until(|| spawner.spawns() == 5).await;
    let _ = failure(&capture).await;

    capture.start().await.expect("start");
    until(|| spawner.spawns() == 6).await;
    let _ = failure(&capture).await;
    let failed = Instant::now();
    until(|| spawner.spawns() == 7).await;

    // The first delay of the backoff is 1 s, with up to 10% of jitter, and the delay after six
    // failures would be 32 s.
    assert!(failed.elapsed() <= Duration::from_millis(1100), "{:?}", failed.elapsed());
}

/// The state after a start shows `Starting` until the helper that the start runs ends or
/// connects, so that `atuin desktop-capture start` reports the outcome of the start.
#[rstest]
#[tokio::test(start_paused = true)]
async fn a_start_shows_starting_until_the_outcome(#[from(paused_home)] home: Home) {
    let spawner = FakeSpawner::new([Script::Missing]);
    let capture = home.open(&home.settings(&["gedit"]), &spawner).await;
    let _ = failure(&capture).await;
    spawner.script_next([Script::SlowNoDesktop, Script::NoDesktop]);

    capture.start().await.expect("start");
    let asked = capture.state();
    tokio::time::sleep(Duration::from_secs(4)).await;
    let meanwhile = capture.state();

    assert_eq!((asked, meanwhile), (CaptureState::Starting, CaptureState::Starting));
    state_becomes(&capture, |state| *state == CaptureState::NoDesktop).await;
}

/// A start while a helper of the quick retries is starting shows `Starting` until that helper's
/// outcome.
#[rstest]
#[tokio::test(start_paused = true)]
async fn a_start_during_a_retry_shows_starting(#[from(paused_home)] home: Home) {
    let spawner = FakeSpawner::new([Script::NoDesktop]);
    let capture = home.open(&home.settings(&["gedit"]), &spawner).await;
    state_becomes(&capture, |state| *state == CaptureState::NoDesktop).await;
    spawner.script_next([Script::SlowNoDesktop, Script::NoDesktop]);
    until(|| spawner.spawns() == 2).await;

    capture.start().await.expect("start");
    let asked = capture.state();

    assert_eq!(asked, CaptureState::Starting);
    assert_eq!(spawner.spawns(), 2);
    state_becomes(&capture, |state| *state == CaptureState::NoDesktop).await;
}

/// A start after a helper's hello, before the helper connects, shows `Starting` until the helper
/// connects, rather than the failure from before the start.
#[rstest]
#[tokio::test(start_paused = true)]
async fn a_start_before_a_helper_connects_shows_starting(#[from(paused_home)] home: Home) {
    let spawner = FakeSpawner::new([Script::Missing]);
    let capture = home.open(&home.settings(&["gedit"]), &spawner).await;
    let _ = failure(&capture).await;
    spawner.script_next([Script::SlowConnect]);
    until(|| spawner.configures().len() == 1).await;
    assert!(matches!(capture.state(), CaptureState::Failed { .. }));

    capture.start().await.expect("start");
    let asked = capture.state();

    assert_eq!(asked, CaptureState::Starting);
    state_becomes(&capture, |state| *state == CaptureState::Running).await;
}

/// A stop leaves a long wait for a desktop as it is, because a stopped capture gains nothing from
/// a helper.
#[rstest]
#[tokio::test(start_paused = true)]
async fn a_stop_leaves_the_wait_for_a_desktop(#[from(paused_home)] home: Home) {
    let spawner = FakeSpawner::new([Script::NoDesktop]);
    let capture = home.open(&home.settings(&["gedit"]), &spawner).await;
    state_becomes(&capture, |state| *state == CaptureState::NoDesktop).await;
    tokio::time::sleep(NO_DESKTOP_PATIENCE + Duration::from_mins(1)).await;
    let before = spawner.spawns();

    capture.stop().await.expect("stop");
    tokio::time::sleep(Duration::from_hours(1)).await;

    assert_eq!(spawner.spawns(), before);
}

/// A start while a helper is running doesn't start another helper.
#[rstest]
#[tokio::test(start_paused = true)]
async fn a_start_while_a_helper_runs_starts_no_other(#[from(paused_home)] home: Home) {
    let spawner = FakeSpawner::new([Script::Serve]);
    let capture = running_capture(&home, &spawner).await;

    capture.start().await.expect("start");
    tokio::time::sleep(Duration::from_secs(10)).await;

    assert_eq!(spawner.spawns(), 1);
    assert_eq!(capture.state(), CaptureState::Running);
}

/// A start doesn't change anything when the settings don't select any application.
#[rstest]
#[tokio::test(start_paused = true)]
async fn a_start_without_apps_starts_no_helper(#[from(paused_home)] home: Home) {
    let spawner = FakeSpawner::new([Script::Serve]);
    let capture = home.open(&home.settings(&[]), &spawner).await;
    state_becomes(&capture, |state| *state == CaptureState::NoApps).await;

    capture.start().await.expect("start");
    tokio::time::sleep(Duration::from_secs(10)).await;

    assert_eq!(spawner.spawns(), 0);
    assert_eq!(capture.state(), CaptureState::NoApps);
}

/// While the watch knows that the user is away from the screen, as the watch on macOS does, no
/// helper starts: not as the daemon starts, not on a reported session, and not on a change of
/// the settings. A helper starts once the user is back.
#[rstest]
#[tokio::test(start_paused = true)]
async fn no_helper_starts_while_the_watch_sees_that_the_user_is_away(
    #[from(paused_home)] home: Home,
) {
    let screen = Screen::new(false);
    let spawner = FakeSpawner::new([Script::Serve]);
    let capture = home.open_watched(&home.settings(&["gedit"]), &spawner, screen.watch()).await;
    state_becomes(&capture, |state| *state == CaptureState::NoDesktop).await;

    capture.session_started();
    capture.apply_settings(&home.settings(&["gedit", "kate"]));
    tokio::time::sleep(Duration::from_hours(24)).await;
    assert_eq!(spawner.spawns(), 0);
    let back = Instant::now();
    screen.set(true);

    state_becomes(&capture, |state| *state == CaptureState::Running).await;
    assert_eq!(spawner.spawns(), 1);
    assert!(back.elapsed() < Duration::from_millis(100), "{:?}", back.elapsed());
}

/// A start when the settings don't select any application doesn't carry over to later settings
/// that select some, so no helper starts while the watch knows that the user is away.
#[rstest]
#[tokio::test(start_paused = true)]
async fn a_start_without_apps_starts_no_helper_later_while_the_user_is_away(
    #[from(paused_home)] home: Home,
) {
    let screen = Screen::new(false);
    let settings = home.settings(&[]);
    let spawner = FakeSpawner::new([Script::Serve]);
    let held = HeldOpen::new(settings.desktop_capture_dir());
    let capture =
        DesktopCapture::open_with(&settings, HOST, Arc::clone(&spawner) as _, screen.watch()).await;
    // The start comes while the store is opening.
    capture.start().await.expect("start");
    drop(held);
    store_opened(&capture, &settings).await;
    state_becomes(&capture, |state| *state == CaptureState::NoApps).await;

    capture.apply_settings(&home.settings(&["gedit"]));
    tokio::time::sleep(Duration::from_hours(1)).await;

    assert_eq!(spawner.spawns(), 0);
    assert_eq!(capture.state(), CaptureState::NoDesktop);
}

/// A helper that doesn't find a desktop while the watch sees the user at the screen starts again
/// with the backoff, so that the helpers stay few while the watch and the helpers disagree.
#[rstest]
#[tokio::test(start_paused = true)]
async fn a_watch_that_disagrees_with_the_helper_brings_the_backoff(
    #[from(paused_home)] home: Home,
) {
    let screen = Screen::new(true);
    let spawner = FakeSpawner::new([Script::NoDesktop]);
    let capture = home.open_watched(&home.settings(&["gedit"]), &spawner, screen.watch()).await;
    state_becomes(&capture, |state| *state == CaptureState::NoDesktop).await;

    tokio::time::sleep(Duration::from_mins(10)).await;

    // 1 + 2 + 4 + 8 + 16 + 32 s, and then a minute each: 15 helpers in 10 minutes.
    let spawns = spawner.spawns();
    assert!((10..=17).contains(&spawns), "{spawns}");
    assert_eq!(capture.state(), CaptureState::NoDesktop);
}

/// How a test fails the helper before it replaces the helper's file.
#[derive(Debug, Clone, Copy)]
enum Failing {
    /// Each helper fails, and the backoff has grown to 16 s.
    Backoff,
    /// The helper is of another build.
    Mismatch,
}

/// A new helper file ends the wait after a failure, because a reinstall can fix the failure.
#[rstest]
#[tokio::test(start_paused = true)]
async fn a_new_helper_file_ends_the_wait_after_a_failure(
    #[from(paused_home)] home: Home,
    #[values(Failing::Backoff, Failing::Mismatch)] failing: Failing,
) {
    let helper = home.helper_file();
    std::fs::write(&helper, "a helper of another build").expect("write the helper");
    let mut settings = home.settings(&["gedit"]);
    settings.desktop_capture.helper_path = Some(helper.clone());
    let scripts = match failing {
        Failing::Backoff => vec![Script::Missing; 5],
        Failing::Mismatch => vec![Script::OtherBuild],
    };
    let failures = scripts.len();
    let spawner = FakeSpawner::new(scripts);
    let capture = home.open(&settings, &spawner).await;
    until(|| spawner.spawns() == failures).await;
    let _ = failure(&capture).await;
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(spawner.spawns(), failures, "the wait lasts");
    spawner.script_next([Script::Serve]);
    let installed = Instant::now();

    std::fs::write(&helper, "the helper of this build").expect("write the helper");

    state_becomes(&capture, |state| *state == CaptureState::Running).await;
    assert_eq!(spawner.spawns(), failures + 1);
    assert!(installed.elapsed() <= helper_file::POLL, "{:?}", installed.elapsed());
}

/// Runs in real time, because the index's pool times out while the clock is paused.
#[rstest]
#[tokio::test]
async fn records_reach_the_store_and_the_index(home: Home) {
    let spawner = FakeSpawner::new([Script::CrashAfterSnapshot, Script::Serve]);
    let capture = home.open(&home.settings(&["gedit"]), &spawner).await;
    let index = capture.index().expect("the index is open");

    let search = async || index.search("zebra", &everything(), 10).await.expect("search");
    let hits = eventually(search, |hits| !hits.is_empty()).await;

    let store = capture.store().expect("the store is open");
    let read = store.read(hits[0].segment, Which::Latest).await.expect("read");
    assert_eq!(read.expect("the snapshot").segment.app, "gedit");
}

#[rstest]
#[tokio::test(start_paused = true)]
async fn stop_pauses_and_survives_restart(#[from(paused_home)] home: Home) {
    let spawner = FakeSpawner::new([Script::Serve]);
    let settings = home.settings(&["gedit"]);
    let capture = running_capture(&home, &spawner).await;

    capture.stop().await.expect("stop");
    assert_eq!(capture.state(), CaptureState::Stopped);
    until(|| spawner.received().contains(&DaemonFrame::Pause)).await;
    capture.shutdown().await;
    drop(capture);
    spawner.forget_received();

    let capture = home.open(&settings, &spawner).await;
    assert_eq!(capture.state(), CaptureState::Stopped);
    until(|| !spawner.received().is_empty()).await;
    let received = spawner.received();
    assert!(
        matches!(received[..], [DaemonFrame::Configure {
            mode: Mode::Paused,
            ..
        }]),
        "{received:?}"
    );

    capture.start().await.expect("start");
    until(|| spawner.received().contains(&DaemonFrame::Resume)).await;
    state_becomes(&capture, |state| *state == CaptureState::Running).await;
    assert_eq!(spawner.spawns(), 2);
}

#[rstest]
#[tokio::test(start_paused = true)]
async fn settings_change_reconfigures_without_respawn(#[from(paused_home)] home: Home) {
    let spawner = FakeSpawner::new([Script::Serve]);
    let capture = running_capture(&home, &spawner).await;

    capture.apply_settings(&home.settings(&["kate"]));
    until(|| spawner.configures().len() == 2).await;

    let configures = spawner.configures();
    assert_eq!(configures[0].scope, Scope::Only(vec!["gedit".to_owned()]));
    assert_eq!(configures[1].scope, Scope::Only(vec!["kate".to_owned()]));
    assert_eq!(spawner.spawns(), 1);
    assert_eq!(capture.state(), CaptureState::Running);
}

/// The helper receives the timing settings, and a change to them reconfigures it.
#[rstest]
#[tokio::test(start_paused = true)]
async fn timing_reaches_the_helper(#[from(paused_home)] home: Home) {
    let spawner = FakeSpawner::new([Script::Serve]);
    let capture = running_capture(&home, &spawner).await;

    let mut settings = home.settings(&["gedit"]);
    settings.desktop_capture.poll_interval = None;
    settings.desktop_capture.read_budget = Duration::from_millis(150);
    capture.apply_settings(&settings);
    until(|| spawner.configures().len() == 2).await;

    let configures = spawner.configures();
    assert_eq!(configures[0].poll_interval, Some(Duration::from_secs(30)));
    assert_eq!(configures[0].read_budget, Duration::from_millis(1500));
    assert_eq!(configures[1].poll_interval, None);
    assert_eq!(configures[1].read_budget, Duration::from_millis(150));
    assert_eq!(spawner.spawns(), 1);
}

#[cfg(unix)]
#[rstest]
#[tokio::test]
async fn helper_command_runs_the_capture_subcommand(home: Home) {
    use std::os::unix::fs::PermissionsExt as _;

    let program = home.dir.path().join("helper");
    std::fs::write(&program, "#!/bin/sh\n[ \"$1\" = capture ] && exit 3\nexit 1\n")
        .expect("write the helper");
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755))
        .expect("make the helper executable");

    let process = HelperCommand::new(None).spawn(&program).expect("spawn");
    let status = process.exit.await.expect("exit");
    assert_eq!(status.code(), Some(3));

    let missing = HelperCommand::new(None).spawn(&home.dir.path().join("missing")).err();
    assert_eq!(missing.map(|err| err.kind()), Some(ErrorKind::NotFound));
}

#[rstest]
#[tokio::test(start_paused = true)]
async fn no_apps_does_not_spawn(#[from(paused_home)] home: Home) {
    let spawner = FakeSpawner::new([Script::Serve]);
    let capture = home.open(&home.settings(&[]), &spawner).await;

    tokio::time::sleep(Duration::from_secs(5)).await;

    assert_eq!(capture.state(), CaptureState::NoApps);
    assert_eq!(spawner.spawns(), 0);
}

/// A stop lasts across the settings that don't select any application, and across the settings
/// that then select one.
#[rstest]
#[case::no_apps(&[], true, CaptureState::NoApps)]
#[case::disabled(&["gedit"], false, CaptureState::Disabled)]
#[tokio::test(start_paused = true)]
async fn stop_shows_while_no_capture_runs(
    #[from(paused_home)] home: Home,
    #[case] apps: &[&str],
    #[case] enabled: bool,
    #[case] idle: CaptureState,
) {
    let spawner = FakeSpawner::new([Script::Serve]);
    let mut settings = home.settings(apps);
    settings.desktop_capture.enabled = enabled;
    let capture = home.open(&settings, &spawner).await;
    assert!(!capture.stopped());

    capture.stop().await.expect("stop");
    assert_eq!(capture.state(), idle);
    assert!(capture.stopped());

    capture.start().await.expect("start");
    assert_eq!(capture.state(), idle);
    assert!(!capture.stopped());

    capture.stop().await.expect("stop");
    capture.apply_settings(&home.settings(&["gedit"]));
    state_becomes(&capture, |state| *state == CaptureState::Stopped).await;
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert!(capture.stopped());
    // The helper that starts for the new apps stays paused.
    assert!(spawner.received().iter().all(|frame| !matches!(frame, DaemonFrame::Configure {
        mode: Mode::Running,
        ..
    })));
}

/// Waits until the store holds the segment and the notification of [`Script::Segment`].
async fn until_stored(store: &DesktopStore) {
    let stored = async || (active_segments(store).await.len(), notifications(store).await);
    eventually(stored, |&(segments, notifications)| segments > 0 && notifications > 0).await;
}

async fn segments_of(store: &DesktopStore, kind: SegmentKind) -> Vec<SegmentSummary> {
    let filter = SegmentFilter {
        kinds: Kinds::Only(vec![kind]),
        ..everything()
    };
    store.segments(filter).await.expect("segments")
}

async fn active_segments(store: &DesktopStore) -> Vec<SegmentSummary> {
    segments_of(store, SegmentKind::Active).await
}

async fn notifications(store: &DesktopStore) -> usize {
    segments_of(store, SegmentKind::Notification).await.len()
}

#[rstest]
#[tokio::test(start_paused = true)]
async fn stop_lasts_when_the_store_fails_to_open(#[from(paused_home)] home: Home) {
    let settings = home.settings(&["gedit"]);
    let spawner = FakeSpawner::new([Script::Serve]);
    let holder = DesktopStore::open(settings.desktop_capture_dir(), HOST).await.expect("open");
    let capture = home.open(&settings, &spawner).await;
    assert!(matches!(capture.state(), CaptureState::Failed { .. }), "{:?}", capture.state());
    assert!(capture.store().is_none());

    capture.stop().await.expect("stop");
    capture.shutdown().await;
    drop(capture);
    drop(holder);

    let capture = home.open(&settings, &spawner).await;
    assert_eq!(capture.state(), CaptureState::Stopped);
    until(|| {
        spawner.received().iter().any(|frame| {
            matches!(frame, DaemonFrame::Configure {
                mode: Mode::Paused,
                ..
            })
        })
    })
    .await;
    assert_eq!(capture.state(), CaptureState::Stopped);
}

/// A helper that starts while capture is stopped is configured as paused, so the helper doesn't
/// read anything.
#[rstest]
#[tokio::test(start_paused = true)]
async fn a_stopped_capture_starts_its_helper_paused(#[from(paused_home)] home: Home) {
    let settings = home.settings(&["gedit"]);
    StopFile::in_dir(settings.desktop_capture_dir()).create().await.expect("stop");
    let spawner = FakeSpawner::new([Script::Segment]);
    let capture = home.open(&settings, &spawner).await;

    until(|| !spawner.received().is_empty()).await;
    // Stopping the helper handles every frame that it sent.
    capture.shutdown().await;

    let received = spawner.received();
    assert!(
        matches!(received[..], [
            DaemonFrame::Configure {
                mode: Mode::Paused,
                ..
            },
            ..
        ]),
        "{received:?}"
    );
    assert!(!received.contains(&DaemonFrame::Pause), "{received:?}");
    assert_eq!(spawner.records_sent(), 0);
    let store = DesktopStore::open(settings.desktop_capture_dir(), HOST).await.expect("reopen");
    assert_eq!(store.segments(everything()).await.expect("segments"), []);
    assert_eq!(notifications(&store).await, 0);
}

/// The shutdown saves the store and closes it, so that the next daemon can open it at once.
#[rstest]
#[tokio::test(start_paused = true)]
async fn shutdown_closes_the_store(#[from(paused_home)] home: Home) {
    let settings = home.settings(&["gedit"]);
    let spawner = FakeSpawner::new([Script::Segment]);
    let capture = home.open(&settings, &spawner).await;
    let store = capture.store().expect("the store is open");
    let segments = async || store.segments(everything()).await.expect("segments");
    // The notification arrives after the segment, and the reopened store must hold both.
    eventually(segments, |segments| segments.len() == 2).await;

    capture.shutdown().await;
    assert!(matches!(
        capture.store().expect("the store").segments(everything()).await,
        Err(StoreError::Closed)
    ));

    let store = DesktopStore::open(settings.desktop_capture_dir(), HOST).await.expect("reopen");
    let segments = store.segments(everything()).await.expect("segments");
    assert_eq!(segments.len(), 2);
    assert_eq!(segments[0].app, "gedit");
}

/// The length prefix of a frame says where the next one starts, so a frame that does not
/// decode is skipped, and the helper goes on.
#[rstest]
#[tokio::test(start_paused = true)]
async fn a_frame_that_does_not_decode_is_skipped(#[from(paused_home)] home: Home) {
    let spawner = FakeSpawner::new([Script::Garbled]);
    let capture = running_capture(&home, &spawner).await;
    let store = capture.store().expect("the store is open");

    eventually(async || notifications(store).await, |&count| count > 0).await;

    assert_eq!(active_segments(store).await.len(), 1);
    assert_eq!(spawner.spawns(), 1);
    assert_eq!(capture.state(), CaptureState::Running);
    capture.shutdown().await;
}

#[rstest]
#[tokio::test(start_paused = true)]
async fn stop_stores_the_end_of_the_open_segment(#[from(paused_home)] home: Home) {
    let spawner = FakeSpawner::new([Script::Segment]);
    let capture = home.open(&home.settings(&["gedit"]), &spawner).await;
    let store = capture.store().expect("the store is open").clone();
    let segments = async || active_segments(&store).await;
    eventually(async || notifications(&store).await, |&count| count > 0).await;
    let open = eventually(segments, |segments| !segments.is_empty()).await;
    assert_eq!(open[0].ended, None);

    capture.stop().await.expect("stop");

    eventually(segments, |segments| segments[0].ended.is_some()).await;
    assert_eq!(spawner.spawns(), 1);
    assert_eq!(capture.state(), CaptureState::Stopped);
}

#[rstest]
#[tokio::test(start_paused = true)]
async fn shutdown_kills_a_helper_that_ignores_it(#[from(paused_home)] home: Home) {
    let spawner = FakeSpawner::new([Script::Stubborn]);
    let capture = running_capture(&home, &spawner).await;

    let asked = Instant::now();
    capture.shutdown().await;

    assert!(spawner.received().contains(&DaemonFrame::Shutdown));
    let waited = asked.elapsed();
    assert!(waited >= SHUTDOWN_GRACE, "{waited:?}");
    assert!(waited < SHUTDOWN_GRACE + Duration::from_millis(100), "{waited:?}");
}

/// How new settings turn capture off.
#[derive(Debug, Clone, Copy)]
enum TurnOff {
    Disable,
    SelectNoApps,
}

/// Settings that turn capture off show in the state at once, before the helper has stopped.
#[rstest]
#[case::disable(TurnOff::Disable, CaptureState::Disabled)]
#[case::select_no_apps(TurnOff::SelectNoApps, CaptureState::NoApps)]
#[tokio::test(start_paused = true)]
async fn settings_that_turn_capture_off_show_at_once(
    #[from(paused_home)] home: Home,
    #[case] turn_off: TurnOff,
    #[case] shown: CaptureState,
) {
    let spawner = FakeSpawner::new([Script::Stubborn]);
    let capture = running_capture(&home, &spawner).await;
    let mut settings = home.settings(&["gedit"]);
    match turn_off {
        TurnOff::Disable => settings.desktop_capture.enabled = false,
        TurnOff::SelectNoApps => settings = home.settings(&[]),
    }

    let changed = Instant::now();
    capture.apply_settings(&settings);
    state_becomes(&capture, |state| *state == shown).await;

    let waited = changed.elapsed();
    assert!(waited < SHUTDOWN_GRACE, "{waited:?}");
    capture.shutdown().await;
}

/// A helper that stops answering pings is killed, its segment ends, and a new helper
/// replaces it.
#[rstest]
#[tokio::test(start_paused = true)]
async fn a_helper_that_stops_answering_is_replaced(#[from(paused_home)] home: Home) {
    let spawner = FakeSpawner::new([Script::Hung, Script::Serve]);
    let capture = running_capture(&home, &spawner).await;
    let connected = Instant::now();

    let error = failure_within(&capture, Duration::from_mins(2)).await;
    assert_eq!(error, STOPPED_ANSWERING);
    let given_up = connected.elapsed();
    assert!(given_up >= PING_INTERVAL + PONG_TIMEOUT, "{given_up:?}");
    assert!(given_up < PING_INTERVAL + PONG_TIMEOUT + Duration::from_secs(1), "{given_up:?}");

    let store = capture.store().expect("the store is open");
    let segments = store.segments(everything()).await.expect("segments");
    assert_eq!(segments.len(), 1);
    assert!(segments[0].ended.is_some(), "{:?}", segments[0]);

    state_becomes(&capture, |state| *state == CaptureState::Running).await;
    assert_eq!(spawner.spawns(), 2);
    // The first delay of the backoff is 1 s, with up to 10% of jitter.
    let replaced = connected.elapsed();
    assert!(replaced < Duration::from_secs(62), "{replaced:?}");
}

/// The run state of a helper lasts as long as the helper, so a window that the helper reads
/// after a stop and a start gets a background segment of its own.
#[rstest]
#[tokio::test(start_paused = true)]
async fn background_reads_after_a_restart_get_their_own_segment(#[from(paused_home)] home: Home) {
    let spawner = FakeSpawner::new([Script::Background]);
    let capture = home.open(&home.settings(&["kate"]), &spawner).await;
    let store = capture.store().expect("the store is open").clone();
    let titles = async || {
        let segments = store.segments(everything()).await.expect("segments");
        segments.into_iter().map(|segment| segment.title).collect::<Vec<_>>()
    };
    eventually(titles, |titles| !titles.is_empty()).await;

    capture.stop().await.expect("stop");
    capture.start().await.expect("start");
    let titles = eventually(titles, |titles| titles.len() >= 2).await;

    assert_eq!(titles, ["window 1", "window 2"]);
    let segments = store.segments(everything()).await.expect("segments");
    assert!(segments.iter().all(|segment| segment.kind == SegmentKind::Background));
    assert_eq!(spawner.spawns(), 1);
}

/// The helper doesn't read anything while the screen is locked, so the state says so until the
/// unlock, and the background segments end at the lock.
#[rstest]
#[tokio::test(start_paused = true)]
async fn a_locked_screen_pauses_capture_until_the_unlock(#[from(paused_home)] home: Home) {
    let spawner = FakeSpawner::new([Script::LocksScreen]);
    let capture = home.open(&home.settings(&["kate"]), &spawner).await;
    let store = capture.store().expect("the store is open").clone();
    let segments = async || store.segments(everything()).await.expect("segments");
    let segments = eventually(segments, |segments| !segments.is_empty()).await;
    assert!(segments.iter().all(|segment| segment.ended.is_none()), "{segments:?}");

    state_becomes(&capture, |state| *state == CaptureState::ScreenLocked).await;
    let segments = store.segments(everything()).await.expect("segments");
    assert_eq!(segments.len(), 1);
    assert!(segments[0].ended.is_some(), "{:?}", segments[0]);

    state_becomes(&capture, |state| *state == CaptureState::Running).await;
    assert_eq!(spawner.spawns(), 1);
}

#[rstest]
#[tokio::test(start_paused = true)]
async fn disabling_stops_the_helper_and_enabling_respawns_it(#[from(paused_home)] home: Home) {
    let spawner = FakeSpawner::new([Script::Serve]);
    let mut settings = home.settings(&["gedit"]);
    let capture = home.open(&settings, &spawner).await;
    state_becomes(&capture, |state| *state == CaptureState::Running).await;

    settings.desktop_capture.enabled = false;
    capture.apply_settings(&settings);
    state_becomes(&capture, |state| *state == CaptureState::Disabled).await;
    until(|| spawner.received().contains(&DaemonFrame::Shutdown)).await;
    assert_eq!(spawner.spawns(), 1);

    settings.desktop_capture.enabled = true;
    capture.apply_settings(&settings);
    state_becomes(&capture, |state| *state == CaptureState::Running).await;
    assert_eq!(spawner.spawns(), 2);
}

/// Runs in real time, because the index's pool times out while the clock is paused.
#[rstest]
#[tokio::test]
async fn delete_during_a_run_deletes_the_open_segment_and_respawns(home: Home) {
    let spawner = FakeSpawner::new([Script::Segment, Script::Serve]);
    let capture = home.open(&home.settings(&["gedit"]), &spawner).await;
    let store = capture.store().expect("the store is open");
    // The notification arrives after the segment, and the delete must find both.
    until_stored(store).await;

    let deleted = capture.delete(everything()).await.expect("delete");
    let deleted_at = Instant::now();

    assert_eq!(deleted.segments.len(), 2);
    assert_eq!(store.segments(everything()).await.expect("segments"), []);
    until(|| spawner.spawns() == 2).await;
    // A respawn after a failure would wait at least 1 s. The deletion itself takes as long as
    // the rewrite of the store, so the wait counts from the deletion's answer.
    assert!(deleted_at.elapsed() < Duration::from_secs(1), "{:?}", deleted_at.elapsed());
    state_becomes(&capture, |state| *state == CaptureState::Running).await;
    capture.shutdown().await;
}

/// Runs in real time, because the index's pool times out while the clock is paused.
#[rstest]
#[tokio::test]
async fn delete_by_id_during_a_run_deletes_the_open_segment(home: Home) {
    let spawner = FakeSpawner::new([Script::Segment, Script::Serve]);
    let capture = home.open(&home.settings(&["gedit"]), &spawner).await;
    let store = capture.store().expect("the store is open");
    // The notification arrives after the segment, and the notification must survive the
    // deletion.
    until_stored(store).await;
    let listed = active_segments(store).await;
    let [segment] = listed.as_slice() else {
        panic!("expected one active segment, got {listed:?}")
    };

    let deleted = capture.delete(segment.key).await.expect("delete");
    let deleted_at = Instant::now();

    assert_eq!(deleted.segments.len(), 1);
    assert_eq!(notifications(store).await, 1);
    assert_eq!(active_segments(store).await, []);
    until(|| spawner.spawns() == 2).await;
    // The wait counts from the deletion's answer, as in the deletion of everything.
    assert!(deleted_at.elapsed() < Duration::from_secs(1), "{:?}", deleted_at.elapsed());
    state_becomes(&capture, |state| *state == CaptureState::Running).await;
    capture.shutdown().await;
}

/// Runs in real time, because the index's pool times out while the clock is paused.
#[rstest]
#[tokio::test]
async fn delete_while_waiting_deletes_in_place(home: Home) {
    let mut settings = home.settings(&["gedit"]);
    settings.desktop_capture.enabled = false;
    let now = SystemTime::now();
    let store = DesktopStore::open(settings.desktop_capture_dir(), HOST).await.expect("open");
    write_all(&store, &mut RunState::default(), [
        started(1, "gedit", "notes", now),
        snapshot(1, now, window("zebra")),
    ])
    .await;
    store.close().await.expect("close");
    drop(store);
    let spawner = FakeSpawner::new([Script::Serve]);
    let capture = home.open(&settings, &spawner).await;
    assert_eq!(capture.state(), CaptureState::Disabled);

    let deleted = capture.delete(everything()).await.expect("delete");

    assert_eq!(deleted.segments.len(), 1);
    let store = capture.store().expect("the store is open");
    assert_eq!(store.segments(everything()).await.expect("segments"), []);
    assert_eq!(spawner.spawns(), 0);
    capture.shutdown().await;
}

/// Runs in real time, because the index's pool times out while the clock is paused.
#[rstest]
#[tokio::test]
async fn delete_while_stopped_deletes_and_keeps_the_new_helper_paused(home: Home) {
    let spawner = FakeSpawner::new([Script::Segment, Script::Serve]);
    let capture = home.open(&home.settings(&["gedit"]), &spawner).await;
    let store = capture.store().expect("the store is open");
    // The notification arrives after the segment, and the delete must find both.
    until_stored(store).await;
    capture.stop().await.expect("stop");
    until(|| spawner.received().contains(&DaemonFrame::Pause)).await;
    spawner.forget_received();

    let deleted = capture.delete(everything()).await.expect("delete");

    assert_eq!(deleted.segments.len(), 2);
    assert_eq!(store.segments(everything()).await.expect("segments"), []);
    assert_eq!(capture.state(), CaptureState::Stopped);
    let paused = |frame: &DaemonFrame| {
        matches!(frame, DaemonFrame::Configure {
            mode: Mode::Paused,
            ..
        })
    };
    until(|| spawner.spawns() == 2 && spawner.received().iter().any(paused)).await;
    let received = spawner.received();
    assert!(matches!(received.last(), Some(frame) if paused(frame)), "{received:?}");
    assert_eq!(capture.state(), CaptureState::Stopped);
    capture.shutdown().await;
}

#[rstest]
#[tokio::test(start_paused = true)]
async fn delete_after_shutdown_fails(#[from(paused_home)] home: Home) {
    let spawner = FakeSpawner::new([Script::Serve]);
    let capture = running_capture(&home, &spawner).await;
    capture.shutdown().await;

    let result = capture.delete(everything()).await;

    assert!(matches!(result, Err(DeleteError::SupervisorEnded)), "{result:?}");
    let result = capture.stop().await;
    assert!(matches!(result, Err(RequestError::SupervisorEnded)), "{result:?}");
}

/// Stops and starts take effect in the order that they were asked, and the state and the stop
/// file agree on the last one.
#[rstest]
#[tokio::test(start_paused = true)]
async fn interleaved_stops_and_starts_agree(#[from(paused_home)] home: Home) {
    let spawner = FakeSpawner::new([Script::Serve]);
    let capture = running_capture(&home, &spawner).await;
    let stop_file = stop_file(&home);

    let (first, second, third) = tokio::join!(capture.stop(), capture.start(), capture.stop());
    for result in <[_; 3]>::from((first, second, third)) {
        result.expect("a request");
    }
    assert_eq!(capture.state(), CaptureState::Stopped);
    assert_eq!(stop_file.requested().await, Requested::Stopped);

    let (first, second) = tokio::join!(capture.stop(), capture.start());
    for result in <[_; 2]>::from((first, second)) {
        result.expect("a request");
    }
    assert_eq!(capture.state(), CaptureState::Running);
    assert_eq!(stop_file.requested().await, Requested::Running);
    until(|| spawner.received().ends_with(&[DaemonFrame::Pause, DaemonFrame::Resume])).await;
}

/// A start whose caller stops waiting still takes effect, in both the state and the stop file.
#[rstest]
#[tokio::test(start_paused = true)]
async fn an_abandoned_start_takes_effect(#[from(paused_home)] home: Home) {
    let spawner = FakeSpawner::new([Script::Serve]);
    let capture = running_capture(&home, &spawner).await;
    capture.stop().await.expect("stop");

    let abandoned = tokio::time::timeout(Duration::ZERO, capture.start()).await;
    assert!(abandoned.is_err(), "the start answered before the supervisor ran");

    state_becomes(&capture, |state| *state == CaptureState::Running).await;
    let stop_file = stop_file(&home);
    eventually(async || stop_file.requested().await, |&requested| requested == Requested::Running)
        .await;
    until(|| spawner.received().contains(&DaemonFrame::Resume)).await;
}

/// A stop file that cannot be written still stops capture in this daemon, and the stop reports
/// the error.
#[rstest]
#[tokio::test(start_paused = true)]
async fn a_stop_file_that_cannot_be_written_still_stops(#[from(paused_home)] home: Home) {
    let spawner = FakeSpawner::new([Script::Serve]);
    let capture = running_capture(&home, &spawner).await;
    // A directory in the place of the stop file keeps the file from being written.
    std::fs::create_dir(home.settings(&[]).desktop_capture_dir().joined("stopped"))
        .expect("create a directory");

    let result = capture.stop().await;

    assert!(matches!(result, Err(RequestError::StopFile(_))), "{result:?}");
    assert_eq!(capture.state(), CaptureState::Stopped);
    until(|| spawner.received().contains(&DaemonFrame::Pause)).await;
}

/// A start that cannot remove the stop file leaves capture stopped, and the helper stays paused.
#[rstest]
#[tokio::test(start_paused = true)]
async fn a_start_that_cannot_remove_the_stop_file_keeps_the_helper_paused(
    #[from(paused_home)] home: Home,
) {
    let spawner = FakeSpawner::new([Script::Serve]);
    let capture = running_capture(&home, &spawner).await;
    capture.stop().await.expect("stop");
    until(|| spawner.received().contains(&DaemonFrame::Pause)).await;
    // A directory that isn't empty, in the place of the stop file, can't be removed as a file.
    let stopped = home.settings(&[]).desktop_capture_dir().joined("stopped");
    std::fs::remove_file(&stopped).expect("remove the stop file");
    std::fs::create_dir(&stopped).expect("create a directory");
    std::fs::write(stopped.join("file"), b"").expect("fill the directory");
    spawner.forget_received();

    let result = capture.start().await;
    tokio::time::sleep(Duration::from_secs(1)).await;

    assert!(matches!(result, Err(RequestError::StopFile(_))), "{result:?}");
    assert_eq!(capture.state(), CaptureState::Stopped);
    assert!(capture.stopped());
    assert_eq!(spawner.received(), []);
    capture.shutdown().await;
}

/// A stop while capture is disabled, before capture ever made a store, doesn't make the
/// daemon open a store.
#[rstest]
#[tokio::test(start_paused = true)]
async fn a_stop_without_a_store_does_not_open_one(#[from(paused_home)] home: Home) {
    let spawner = FakeSpawner::new([Script::Serve]);
    let mut settings = home.settings(&["gedit"]);
    settings.desktop_capture.enabled = false;
    let capture = home.open(&settings, &spawner).await;

    capture.stop().await.expect("stop");
    settings.desktop_capture.exclude.push("kate".to_owned());
    capture.apply_settings(&settings);
    // The supervisor answers the second stop after it has handled the settings.
    capture.stop().await.expect("stop again");

    assert!(capture.store().is_none());
    assert!(!home.dir.path().join("desktop-capture-index.sqlite").exists());
    assert_eq!(capture.state(), CaptureState::Disabled);
    capture.shutdown().await;
}

/// The capture opens without waiting for the store, so that the daemon serves its other clients
/// while a large store is recovering.
#[rstest]
#[tokio::test(start_paused = true)]
async fn opening_does_not_wait_for_the_store(#[from(paused_home)] home: Home) {
    let settings = home.settings(&["gedit"]);
    let spawner = FakeSpawner::new([Script::Serve]);
    let holder = DesktopStore::open(settings.desktop_capture_dir(), HOST).await.expect("open");

    let capture = DesktopCapture::open_with(&settings, HOST, spawner as _, unwatched()).await;

    assert_eq!(capture.state(), CaptureState::Starting);
    assert!(capture.store().is_none());
    store_opened(&capture, &settings).await;
    assert!(matches!(capture.state(), CaptureState::Failed { .. }), "{:?}", capture.state());
    drop(holder);
    capture.shutdown().await;
}

/// A stop takes effect while the store is opening, and a shutdown during the open doesn't wait for
/// the open.
#[rstest]
#[tokio::test(start_paused = true)]
async fn a_stop_and_a_shutdown_do_not_wait_for_the_store(#[from(paused_home)] home: Home) {
    let settings = home.settings(&["gedit"]);
    let spawner = FakeSpawner::new([Script::Serve]);
    let held = HeldOpen::new(settings.desktop_capture_dir());
    let capture = DesktopCapture::open_with(&settings, HOST, spawner as _, unwatched()).await;

    let stopped = timeout(PATIENCE, capture.stop()).await;
    stopped.expect("the stop doesn't wait for the store").expect("stop");
    assert_eq!(capture.state(), CaptureState::Stopped);
    let begun = Instant::now();
    capture.shutdown().await;

    assert!(begun.elapsed() < SHUTDOWN_TIMEOUT, "{:?}", begun.elapsed());
    assert!(capture.store().is_none());
    drop(held);
}

/// A deletion that arrives while the store is opening runs once the store is open.
///
/// Runs in real time, because the index's pool times out while the clock is paused.
#[rstest]
#[tokio::test]
async fn a_deletion_during_the_open_runs_once_the_store_is_open(home: Home) {
    let settings = home.settings(&["gedit"]);
    let spawner = FakeSpawner::new([Script::Serve]);
    let held = HeldOpen::new(settings.desktop_capture_dir());
    let capture = DesktopCapture::open_with(&settings, HOST, spawner as _, unwatched()).await;
    let deletion = capture.delete(everything());
    tokio::pin!(deletion);

    let waited = timeout(Duration::from_millis(100), &mut deletion).await;
    assert!(waited.is_err(), "the deletion waits for the store: {waited:?}");
    drop(held);
    let deleted = deletion.await.expect("delete");

    assert!(deleted.is_empty());
    capture.shutdown().await;
}

/// A panic of the supervisor shows as `Failed`, and later requests fail. A stop still writes the
/// stop file, so that the next daemon keeps capture stopped.
#[rstest]
#[tokio::test(start_paused = true)]
async fn a_panic_shows_as_failed(#[from(paused_home)] home: Home) {
    let capture = open_with(&home, Arc::new(PanickingSpawner)).await;

    let error = failure(&capture).await;

    assert!(error.ends_with("panicked: the spawner broke"), "{error}");
    let result = capture.start().await;
    assert!(matches!(result, Err(RequestError::SupervisorEnded)), "{result:?}");
    assert_eq!(stop_file(&home).requested().await, Requested::Running);
    let result = capture.stop().await;
    assert!(matches!(result, Err(RequestError::SupervisorEnded)), "{result:?}");
    assert_eq!(stop_file(&home).requested().await, Requested::Stopped);
    assert!(capture.stopped());
    let result = capture.delete(everything()).await;
    assert!(matches!(result, Err(DeleteError::SupervisorEnded)), "{result:?}");
    capture.shutdown().await;
}

/// Stops and starts take effect while the helper is being prepared, and a shutdown abandons the
/// preparation.
#[rstest]
#[tokio::test(start_paused = true)]
async fn requests_run_while_the_helper_is_prepared(#[from(paused_home)] home: Home) {
    let spawner = Arc::new(HangingSpawner::default());
    let capture = open_with(&home, Arc::<HangingSpawner>::clone(&spawner)).await;
    until(|| spawner.prepares() == 1).await;
    let asked = Instant::now();

    capture.stop().await.expect("stop");
    assert_eq!(capture.state(), CaptureState::Stopped);
    capture.start().await.expect("start");
    assert_eq!(capture.state(), CaptureState::Starting);
    capture.shutdown().await;

    assert!(asked.elapsed() < Duration::from_secs(1), "{:?}", asked.elapsed());
    assert_eq!((spawner.prepares(), spawner.dropped()), (1, 1));
}

/// Runs in real time, because the index's pool times out while the clock is paused.
#[rstest]
#[tokio::test]
async fn delete_while_the_helper_is_prepared_deletes_and_prepares_again(home: Home) {
    let spawner = Arc::new(HangingSpawner::default());
    let capture = open_with(&home, Arc::<HangingSpawner>::clone(&spawner)).await;
    until(|| spawner.prepares() == 1).await;

    let deleted = capture.delete(everything()).await.expect("delete");

    assert!(deleted.segments.is_empty());
    until(|| spawner.prepares() == 2).await;
    assert_eq!(spawner.dropped(), 1);
    capture.shutdown().await;
}

/// A preparation that takes too long fails, and is tried again after the backoff.
#[rstest]
#[tokio::test(start_paused = true)]
async fn a_hung_preparation_fails(#[from(paused_home)] home: Home) {
    let spawner = Arc::new(HangingSpawner::default());
    let capture = open_with(&home, Arc::<HangingSpawner>::clone(&spawner)).await;
    let started = Instant::now();

    let error = failure_within(&capture, PREPARE_TIMEOUT * 2).await;

    assert_eq!(
        error,
        format!("signing the helper took longer than {} s", PREPARE_TIMEOUT.as_secs())
    );
    assert!(started.elapsed() >= PREPARE_TIMEOUT, "{:?}", started.elapsed());
    assert_eq!(spawner.dropped(), 1);
    until(|| spawner.prepares() == 2).await;
    capture.shutdown().await;
}

/// A store that fails to open is opened again with the backoff, and capture then starts.
#[rstest]
#[tokio::test(start_paused = true)]
async fn a_store_that_fails_to_open_is_opened_again(#[from(paused_home)] home: Home) {
    let settings = home.settings(&["gedit"]);
    let spawner = FakeSpawner::new([Script::Serve]);
    let holder = DesktopStore::open(settings.desktop_capture_dir(), HOST).await.expect("open");
    let capture = home.open(&settings, &spawner).await;

    let error = failure(&capture).await;
    assert!(error.starts_with("could not open the desktop capture store"), "{error}");
    assert!(capture.store().is_none());
    drop(holder);

    state_becomes(&capture, |state| *state == CaptureState::Running).await;
    assert!(capture.store().is_some());
    capture.shutdown().await;
}

/// A stop keeps the stop file and answers before it tells the helper. A helper that doesn't take
/// a frame within `SEND_TIMEOUT` counts as hung, and a new helper replaces it.
#[rstest]
#[tokio::test(start_paused = true)]
async fn a_stop_does_not_wait_for_a_helper_that_stopped_reading(#[from(paused_home)] home: Home) {
    let spawner = FakeSpawner::new([Script::Deaf, Script::Serve]);
    let capture = running_capture(&home, &spawner).await;
    let asked = Instant::now();

    tokio::time::timeout(Duration::from_secs(1), capture.stop())
        .await
        .expect("the stop answers at once")
        .expect("stop");

    assert_eq!(stop_file(&home).requested().await, Requested::Stopped);
    until(|| spawner.spawns() == 2).await;
    assert!(asked.elapsed() >= SEND_TIMEOUT, "{:?}", asked.elapsed());
    let paused = |frame: &DaemonFrame| {
        matches!(frame, DaemonFrame::Configure {
            mode: Mode::Paused,
            ..
        })
    };
    until(|| spawner.received().iter().any(paused)).await;
    assert_eq!(capture.state(), CaptureState::Stopped);
    capture.shutdown().await;
}

/// A store that fails to open at startup is opened again after the backoff, and its failure
/// leaves the state once the store opens, while the helper is still being prepared.
#[rstest]
#[tokio::test(start_paused = true)]
async fn a_store_that_opens_late_clears_its_failure(#[from(paused_home)] home: Home) {
    let settings = home.settings(&["gedit"]);
    let spawner = Arc::new(HangingSpawner::default());
    let holder = DesktopStore::open(settings.desktop_capture_dir(), HOST).await.expect("open");
    let started = Instant::now();
    let capture = DesktopCapture::open_with(
        &settings,
        HOST,
        Arc::<HangingSpawner>::clone(&spawner) as _,
        unwatched(),
    )
    .await;
    store_opened(&capture, &settings).await;
    assert!(matches!(capture.state(), CaptureState::Failed { .. }), "{:?}", capture.state());
    drop(holder);

    state_becomes(&capture, |state| *state == CaptureState::Starting).await;

    // The first delay of the backoff is 1 s, with up to 10% of jitter.
    assert!(started.elapsed() >= Duration::from_millis(900), "{:?}", started.elapsed());
    until(|| spawner.prepares() == 1).await;
    assert_eq!(capture.state(), CaptureState::Starting);
    capture.shutdown().await;
}

/// A run ends a background segment that it has not written to for an hour, without waiting for
/// another read of the segment's window.
#[rstest]
#[tokio::test(start_paused = true)]
async fn a_run_ends_its_idle_background_segments(#[from(paused_home)] home: Home) {
    let spawner = FakeSpawner::new([Script::StaleBackground]);
    let capture = home.open(&home.settings(&["kate"]), &spawner).await;
    let store = capture.store().expect("the store is open").clone();
    let segments = async || store.segments(everything()).await.expect("segments");
    let read = eventually(segments, |segments| !segments.is_empty()).await;
    assert_eq!(read[0].ended, None);

    tokio::time::advance(IDLE_CHECK).await;

    // A read that races the end's commit sees the segment as ended at its last record for a
    // moment, so the check waits for the stored end. The segment's only record is its start.
    let stored_end = |segments: &Vec<SegmentSummary>| {
        segments[0].ended.is_some_and(|end| end > segments[0].started)
    };
    let ended = eventually(segments, stored_end).await;
    // The store ends an idle segment an hour after its last record.
    assert_eq!(ended[0].ended, Some(ended[0].started + Duration::from_hours(1)));
    assert_eq!(spawner.spawns(), 1);
    capture.shutdown().await;
}

/// What interrupts a run while the run is reading background windows.
#[derive(Debug, Clone, Copy)]
enum Interruption {
    Stop,
    /// A deletion, which restarts the helper.
    Delete,
}

/// A stop, and the restart that a deletion causes, end the run's background segments when they
/// happen. A window that the run read long ago and that hasn't changed since lasts until then.
///
/// Runs in real time, because the index's pool times out while the clock is paused.
#[rstest]
#[tokio::test]
async fn an_interruption_ends_the_background_segments(
    home: Home,
    #[values(Interruption::Stop, Interruption::Delete)] interruption: Interruption,
) {
    let spawner = FakeSpawner::new([Script::StaleBackground]);
    let capture = home.open(&home.settings(&["kate"]), &spawner).await;
    let store = capture.store().expect("the store is open").clone();
    let segments = async || store.segments(everything()).await.expect("segments");
    let read = eventually(segments, |segments| !segments.is_empty()).await;
    assert_eq!(read[0].ended, None);

    // The store keeps whole milliseconds.
    let interrupted = SystemTime::now() - Duration::from_millis(1);
    match interruption {
        Interruption::Stop => capture.stop().await.expect("stop"),
        Interruption::Delete => {
            let unrelated = SegmentFilter {
                app: Some("gedit".to_owned()),
                ..everything()
            };
            capture.delete(unrelated).await.expect("delete");
        }
    }

    // A read that races the end's commit sees the segment as ended at its last record for a
    // moment, so the check waits for the stored end. A segment that stays open, or that ends at
    // its read two hours ago, never meets the condition.
    eventually(segments, |segments| segments[0].ended.is_some_and(|end| end >= interrupted)).await;
    capture.shutdown().await;
}
