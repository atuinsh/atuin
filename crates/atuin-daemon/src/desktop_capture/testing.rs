//! A fake helper and other fixtures for the tests of desktop capture.

mod fake_helper;
pub mod records;

use std::fmt::Debug;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use atuin_client::settings::{Apps, DesktopCapture as DesktopCaptureSettings, Settings};
use atuin_common::path::AbsPathBuf;
use atuin_domain::record::HostId;
use futures::future::{self, BoxFuture};
use rstest::fixture;
use tempfile::TempDir;
use tokio::sync::{Semaphore, watch};
use tokio::time::Instant;
use uuid::Uuid;

pub use self::fake_helper::{FakeSpawner, Script};
use super::index::DesktopIndex;
use super::session_watch::{Desktop, SessionWatch};
use super::store::{ContentKey, DesktopStore};
use super::{CaptureState, DesktopCapture, Spawn};

pub const HOST: HostId = HostId(Uuid::from_u128(0x42));
pub const HELPER: &str = "/nonexistent/atuin-desktop-capture";
/// How long a test waits for something that should happen.
pub const PATIENCE: Duration = Duration::from_secs(30);

/// A temporary home directory for a desktop capture.
///
/// A test that deletes captured content shuts its capture down before the `Home` drops. fjall's
/// workers can still flush the store after a deletion, and a flush into a removed directory
/// crashes the worker.
pub struct Home {
    pub dir: TempDir,
}

#[fixture]
pub fn home() -> Home {
    Home {
        dir: tempfile::tempdir().expect("tempdir"),
    }
}

/// A home for a test that pauses the clock. The captures in the home open without the search
/// index.
///
/// SQLite runs the index's statements on threads of its own, which a paused clock doesn't wait
/// for. While the supervisor waits for the index, the clock would jump to the next timer, such
/// as the helper's ping or the test's patience. The store's work runs on tokio's blocking
/// threads, and the clock waits for those.
#[fixture]
pub fn paused_home() -> Home {
    let home = home();
    UNINDEXED.lock().push(home.dir.path().to_owned());
    home
}

impl Drop for Home {
    fn drop(&mut self) {
        UNINDEXED.lock().retain(|dir| dir != self.dir.path());
    }
}

/// The data directories whose captures open without the search index.
static UNINDEXED: parking_lot::Mutex<Vec<PathBuf>> = parking_lot::Mutex::new(Vec::new());

/// Whether the captures with the data directory `data_dir` open the search index.
pub fn opens_index(data_dir: &Path) -> bool {
    !UNINDEXED.lock().iter().any(|dir| dir == data_dir)
}

impl Home {
    pub fn settings(&self, apps: &[&str]) -> Settings {
        Settings {
            data_dir: AbsPathBuf::new(self.dir.path()).expect("an absolute path"),
            desktop_capture: DesktopCaptureSettings {
                enabled: true,
                apps: Apps::Listed(apps.iter().map(|&app| app.to_owned()).collect()),
                helper_path: Some(PathBuf::from(HELPER)),
                ..DesktopCaptureSettings::default()
            },
            ..Settings::default()
        }
    }

    /// Opens a capture whose watch never sees a session start, and waits until the capture opens
    /// its store or fails, as [`store_opened`] does.
    pub async fn open(&self, settings: &Settings, spawner: &Arc<FakeSpawner>) -> DesktopCapture {
        self.open_watched(settings, spawner, unwatched()).await
    }

    /// Opens a capture with the session watch `watch`, as [`Self::open`] does.
    pub async fn open_watched(
        &self,
        settings: &Settings,
        spawner: &Arc<FakeSpawner>,
        watch: Box<dyn SessionWatch>,
    ) -> DesktopCapture {
        let spawner: Arc<dyn Spawn> = Arc::<FakeSpawner>::clone(spawner);
        let capture = DesktopCapture::open_with(settings, HOST, spawner, watch).await;
        store_opened(&capture, settings).await;
        capture
    }

    /// Opens a capture whose watch never sees a session start, and returns at once, so the
    /// capture may still be opening its store.
    pub async fn open_unsettled(
        &self,
        settings: &Settings,
        spawner: &Arc<FakeSpawner>,
    ) -> DesktopCapture {
        let spawner: Arc<dyn Spawn> = Arc::<FakeSpawner>::clone(spawner);
        DesktopCapture::open_with(settings, HOST, spawner, unwatched()).await
    }

    /// The path of a helper file in the home, for the tests that change the helper's file.
    pub fn helper_file(&self) -> PathBuf {
        self.dir.path().join("atuin-desktop-capture")
    }
}

/// A watch that never sees a session start.
pub fn unwatched() -> Box<dyn SessionWatch> {
    SessionStarts::default().watch()
}

/// Starts the desktop sessions that the watches from [`Self::watch`] see.
#[derive(Default)]
pub struct SessionStarts {
    /// How many sessions started.
    started: watch::Sender<u64>,
}

impl SessionStarts {
    /// A watch that sees the sessions that [`Self::start`] starts after the watch's mark.
    pub fn watch(&self) -> Box<dyn SessionWatch> {
        Box::new(FakeSessionWatch {
            started: self.started.subscribe(),
            marked: 0,
        })
    }

    pub fn start(&self) {
        self.started.send_modify(|started| *started += 1);
    }
}

/// A watch that a [`SessionStarts`] drives.
struct FakeSessionWatch {
    started: watch::Receiver<u64>,
    /// How many sessions had started at the last mark.
    marked: u64,
}

impl SessionWatch for FakeSessionWatch {
    fn desktop(&self) -> Desktop {
        Desktop::Unknown
    }

    fn mark(&mut self) {
        self.marked = *self.started.borrow();
    }

    /// Never resolves once the [`SessionStarts`] dropped.
    fn started(&mut self) -> BoxFuture<'_, ()> {
        let marked = self.marked;
        Box::pin(async move {
            if self.started.wait_for(|&started| started > marked).await.is_err() {
                future::pending::<()>().await;
            }
        })
    }
}

/// Puts the user at the screen or away, for the watches from [`Self::watch`], which know whether
/// a helper finds a desktop, as the watch on macOS does.
pub struct Screen {
    at_screen: watch::Sender<bool>,
}

impl Screen {
    pub fn new(at_screen: bool) -> Self {
        Self {
            at_screen: watch::Sender::new(at_screen),
        }
    }

    pub fn watch(&self) -> Box<dyn SessionWatch> {
        Box::new(FakeScreenWatch {
            at_screen: self.at_screen.subscribe(),
        })
    }

    pub fn set(&self, at_screen: bool) {
        self.at_screen.send_replace(at_screen);
    }
}

/// A watch that a [`Screen`] drives.
struct FakeScreenWatch {
    at_screen: watch::Receiver<bool>,
}

impl SessionWatch for FakeScreenWatch {
    fn desktop(&self) -> Desktop {
        if *self.at_screen.borrow() {
            Desktop::Present
        } else {
            Desktop::Absent
        }
    }

    fn mark(&mut self) {}

    /// Never resolves once the [`Screen`] dropped.
    fn started(&mut self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            if self.at_screen.wait_for(|&at_screen| at_screen).await.is_err() {
                future::pending::<()>().await;
            }
        })
    }
}

/// Waits until `capture` opens its store, fails to open it, or fails, when `settings` needs a
/// store.
///
/// The capture opens its store on a blocking thread after it starts. The wait yields without a
/// timer, because a paused clock would otherwise move on while the store opens, and the
/// timeouts of the test would expire.
pub async fn store_opened(capture: &DesktopCapture, settings: &Settings) {
    let needs_store =
        settings.desktop_capture.enabled || super::store_exists(&settings.desktop_capture_dir());
    let attempts = capture.open_attempts();
    let attempted = || attempts.as_ref().is_some_and(|attempts| *attempts.borrow() > 0);
    while needs_store
        && capture.store().is_none()
        && !attempted()
        && !matches!(capture.state(), CaptureState::Failed { .. })
    {
        tokio::task::yield_now().await;
    }
}

/// The store directories whose opens wait, each with the semaphore that the opens wait on.
static HELD_OPENS: parking_lot::Mutex<Vec<(PathBuf, Arc<Semaphore>)>> =
    parking_lot::Mutex::new(Vec::new());

/// Keeps a capture from opening its store in a directory until the `HeldOpen` drops.
pub struct HeldOpen {
    dir: PathBuf,
    gate: Arc<Semaphore>,
}

impl HeldOpen {
    pub fn new(dir: PathBuf) -> Self {
        let gate = Arc::new(Semaphore::new(0));
        HELD_OPENS.lock().push((dir.clone(), Arc::clone(&gate)));
        Self { dir, gate }
    }
}

impl Drop for HeldOpen {
    fn drop(&mut self) {
        HELD_OPENS.lock().retain(|(dir, _)| *dir != self.dir);
        self.gate.close();
    }
}

/// Waits while a [`HeldOpen`] holds the opens of the store in `dir`.
pub async fn held_open(dir: &Path) {
    let gate =
        HELD_OPENS.lock().iter().find(|(held, _)| held == dir).map(|(_, gate)| Arc::clone(gate));
    if let Some(gate) = gate {
        let _ = gate.acquire().await;
    }
}

/// A temporary directory for a store and an index.
#[fixture]
pub fn dir() -> TempDir {
    tempfile::tempdir().expect("tempdir")
}

/// The files under `dir` whose bytes hold `needle`.
pub fn files_holding(dir: &Path, needle: &str) -> Vec<PathBuf> {
    fn walk(dir: &Path, needle: &[u8], found: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).expect("read a directory") {
            let path = entry.expect("a directory entry").path();
            if path.is_dir() {
                walk(&path, needle, found);
            } else if std::fs::read(&path)
                .expect("read a file")
                .windows(needle.len())
                .any(|window| window == needle)
            {
                found.push(path);
            }
        }
    }
    let mut found = Vec::new();
    walk(dir, needle.as_bytes(), &mut found);
    found
}

/// Opens the store in `dir`, or opens it again.
pub async fn store_in(dir: &TempDir) -> DesktopStore {
    DesktopStore::open(dir.path().join("store"), HOST).await.expect("open the store")
}

/// Opens the index in `dir`, or opens it again.
pub async fn index_in(dir: &TempDir) -> DesktopIndex {
    DesktopIndex::open(&dir.path().join("index.sqlite")).await.expect("open the index")
}

/// A store and an index in a temporary directory.
pub struct Opened {
    /// Holds the directory until the test ends. A test that destructures `Opened` binds this
    /// field to `_dir`, because a pattern with `..` would drop the directory at once.
    pub dir: TempDir,
    pub store: DesktopStore,
    pub index: DesktopIndex,
}

#[fixture]
pub async fn opened(dir: TempDir) -> Opened {
    let store = store_in(&dir).await;
    let index = index_in(&dir).await;
    Opened { dir, store, index }
}

/// A store alone in a temporary directory.
pub struct OpenedStore {
    /// Holds the directory until the test ends.
    pub dir: TempDir,
    pub store: DesktopStore,
}

#[fixture]
pub async fn opened_store(dir: TempDir) -> OpenedStore {
    let store = store_in(&dir).await;
    OpenedStore { dir, store }
}

/// An index alone in a temporary directory.
pub struct OpenedIndex {
    /// Holds the directory until the test ends.
    pub dir: TempDir,
    pub index: DesktopIndex,
}

#[fixture]
pub async fn opened_index(dir: TempDir) -> OpenedIndex {
    let index = index_in(&dir).await;
    OpenedIndex { dir, index }
}

/// The contents that the index finds for `query`, best first, up to 10.
pub async fn indexed_keys(index: &DesktopIndex, query: &str) -> Vec<ContentKey> {
    index.search(query, &records::everything(), 10).await.expect("search")
}

/// Waits until the state of `capture` satisfies `wanted`, and returns that state.
pub async fn state_becomes(
    capture: &DesktopCapture,
    wanted: impl Fn(&CaptureState) -> bool,
) -> CaptureState {
    state_becomes_within(capture, PATIENCE, wanted).await
}

/// Waits up to `patience` until the state of `capture` satisfies `wanted`, and returns that
/// state.
pub async fn state_becomes_within(
    capture: &DesktopCapture,
    patience: Duration,
    wanted: impl Fn(&CaptureState) -> bool,
) -> CaptureState {
    let mut published = capture.watch();
    tokio::time::timeout(patience, published.wait_for(|published| wanted(&published.state)))
        .await
        .unwrap_or_else(|_| panic!("the state stayed {:?}", capture.state()))
        .expect("the supervisor is running")
        .state
        .clone()
}

/// Waits until `condition` holds.
pub async fn until(condition: impl Fn() -> bool) {
    eventually(async || condition(), |&held| held).await;
}

/// Polls `probe` until `wanted` holds of the value that `probe` returns, and returns that value.
pub async fn eventually<T: Debug>(
    mut probe: impl AsyncFnMut() -> T,
    wanted: impl Fn(&T) -> bool,
) -> T {
    let deadline = Instant::now() + PATIENCE;
    loop {
        let value = probe().await;
        if wanted(&value) {
            return value;
        }
        assert!(
            Instant::now() < deadline,
            "the condition never held: the last value was {value:?}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}
