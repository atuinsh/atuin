//! Desktop capture: the records that the `atuin-desktop-capture` helper reads from the desktop.

pub mod index;
pub mod reconciler;
mod retention;
pub mod search;
mod session_watch;
pub mod signing;
mod stop_file;
pub mod store;
mod supervisor;
#[cfg(test)]
pub mod testing;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use atuin_client::settings::Settings;
use atuin_domain::record::HostId;
use tokio::sync::watch;
use tracing::{error, warn};

use self::index::DesktopIndex;
use self::session_watch::SessionWatch;
use self::signing::Signing;
pub use self::stop_file::{Requested, StopFile, StopFileError};
use self::store::{Deleted, DesktopStore, Selection};
use self::supervisor::Supervisor;
pub use self::supervisor::{DeleteError, HelperCommand, Opened, RequestError, Spawn};

/// The name of the helper's executable, without the platform's extension.
pub const HELPER_NAME: &str = "atuin-desktop-capture";

/// The error of a request that needs the store while the store is not open.
#[derive(Debug, thiserror::Error)]
#[error(
    "the desktop capture store is not open: capture was never enabled, or the store is still \
     opening, or the store failed to open"
)]
pub struct StoreNotOpen;

/// Where desktop capture stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureState {
    /// The `enabled` setting is off.
    Disabled,
    /// The `apps` setting doesn't select any application.
    NoApps,
    /// The user ran `atuin desktop-capture stop`.
    Stopped,
    Starting,
    /// The helper is connected and capturing.
    Running,
    /// The desktop refuses the helper's reads until the user grants access.
    NeedsPermission,
    /// No desktop session is running, so the helper is waiting for one.
    NoDesktop,
    /// The screen is locked, so the helper doesn't read anything until the screen unlocks.
    ScreenLocked,
    Failed {
        error: String,
    },
}

/// What desktop capture publishes: its state, and whether the user stopped capture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Published {
    pub state: CaptureState,
    /// What the user asked for, which holds in every state, including `Disabled` and `NoApps`.
    pub requested: Requested,
}

/// Desktop capture in the daemon: the store and index, and the supervisor of the helper.
pub struct DesktopCapture {
    published: watch::Receiver<Published>,
    control: Control,
}

/// What carries out `stop`, `start`, and deletions.
enum Control {
    Supervisor(Supervisor),
    /// The stop file alone, for a capture that doesn't run a supervisor.
    StopFile(StopFile),
}

impl DesktopCapture {
    /// Starts desktop capture, which opens the store and starts the helper when the settings
    /// enable capture.
    ///
    /// The store opens after this returns. If `stop` or `start` runs before the store is open,
    /// the request takes effect at once, and a deletion waits until the store is open. A store
    /// that cannot open shows as `Failed` until a later attempt opens it.
    pub async fn open(settings: &Settings) -> Self {
        match settings.host_id().await {
            Ok(host) => {
                let spawner = Arc::new(HelperCommand::new(Signing::for_platform()));
                Self::open_with(settings, host, spawner, session_watch::for_platform()).await
            }
            Err(err) => Self::failed(
                format!("could not read this host's id: {err}"),
                StopFile::in_dir(settings.desktop_capture_dir()),
            ),
        }
    }

    /// A capture with no store that never captures. Its stop file is at a path that it cannot
    /// create, so its `stop` fails.
    #[cfg(test)]
    #[must_use]
    pub fn nop() -> Self {
        Self {
            published: watch::channel(Published {
                state: CaptureState::Disabled,
                requested: Requested::Running,
            })
            .1,
            control: Control::StopFile(StopFile::in_dir(PathBuf::from(
                "/nonexistent/desktop-capture",
            ))),
        }
    }

    /// A capture that serves nothing. Its `stop` and `start` still keep the stop file, so that a
    /// stop lasts until a later daemon starts capture.
    fn failed(error: String, stop_file: StopFile) -> Self {
        error!(%error, "desktop capture is unavailable");
        Self {
            published: watch::channel(Published {
                state: CaptureState::Failed { error },
                requested: Requested::Running,
            })
            .1,
            control: Control::StopFile(stop_file),
        }
    }

    async fn open_with(
        settings: &Settings,
        host: HostId,
        spawner: Arc<dyn Spawn>,
        sessions: Box<dyn SessionWatch>,
    ) -> Self {
        let stop_file = StopFile::in_dir(settings.desktop_capture_dir());
        let supervisor = Supervisor::start(settings, host, spawner, sessions, stop_file).await;
        Self {
            published: supervisor.published(),
            control: Control::Supervisor(supervisor),
        }
    }

    const fn supervisor(&self) -> Option<&Supervisor> {
        match &self.control {
            Control::Supervisor(supervisor) => Some(supervisor),
            Control::StopFile(_) => None,
        }
    }

    /// Where capture stands. A store that fails every operation shows as `Failed`.
    #[must_use]
    pub fn state(&self) -> CaptureState {
        if let Some(error) = self.store().and_then(DesktopStore::failure) {
            return CaptureState::Failed {
                error: error.to_string(),
            };
        }
        self.published.borrow().state.clone()
    }

    #[must_use]
    pub fn is_running(&self) -> bool {
        self.state() == CaptureState::Running
    }

    /// Whether the user stopped capture, which holds in every state, including `Disabled` and
    /// `NoApps`.
    ///
    /// A capture that doesn't run a supervisor reports `false`.
    #[must_use]
    pub fn stopped(&self) -> bool {
        self.published.borrow().requested == Requested::Stopped
    }

    /// The published state, for tests that wait for a state.
    #[cfg(test)]
    #[must_use]
    pub fn watch(&self) -> watch::Receiver<Published> {
        self.published.clone()
    }

    /// Counts the attempts to open the store that have ended. `None` for a capture that doesn't
    /// run a supervisor.
    #[cfg(test)]
    #[must_use]
    pub fn open_attempts(&self) -> Option<watch::Receiver<u32>> {
        self.supervisor().map(Supervisor::open_attempts)
    }

    /// Stops capture until [`start`](Self::start), across daemon restarts.
    ///
    /// Capture stops in this daemon even when the stop cannot be kept across restarts, which
    /// the error then reports.
    pub async fn stop(&self) -> Result<(), RequestError> {
        self.request(Requested::Stopped).await
    }

    /// Undoes [`stop`](Self::stop), and starts a helper at once if no helper is running and the
    /// settings let a helper capture.
    pub async fn start(&self) -> Result<(), RequestError> {
        self.request(Requested::Running).await
    }

    /// Waits up to `patience` until the state isn't `Starting`: until the helper that a
    /// [`start`](Self::start) ran is capturing or has ended.
    pub async fn settled(&self, patience: Duration) {
        let mut published = self.published.clone();
        let starting = |published: &Published| published.state == CaptureState::Starting;
        let _ = tokio::time::timeout(patience, published.wait_for(|p| !starting(p))).await;
    }

    async fn request(&self, requested: Requested) -> Result<(), RequestError> {
        match &self.control {
            Control::Supervisor(supervisor) => supervisor.request(requested).await,
            Control::StopFile(file) => Ok(file.set(requested).await?),
        }
    }

    /// Deletes the captured segments that `selection` selects, and their contents from the search
    /// index.
    ///
    /// A running helper stops for the deletion, and a new helper starts once the deletion ends.
    pub async fn delete(&self, selection: impl Into<Selection>) -> Result<Deleted, DeleteError> {
        match self.supervisor() {
            Some(supervisor) => supervisor.delete(selection.into()).await,
            None => Err(DeleteError::NotOpen(StoreNotOpen)),
        }
    }

    /// Applies new settings to the running capture.
    pub fn apply_settings(&self, settings: &Settings) {
        if let Some(supervisor) = self.supervisor() {
            supervisor.apply_settings(settings);
        }
    }

    /// Starts a helper soon if capture is waiting for a desktop session, because a session has
    /// started.
    pub fn session_started(&self) {
        if let Some(supervisor) = self.supervisor() {
            supervisor.session_started();
        }
    }

    /// Stops the helper and the background tasks, for the daemon's shutdown.
    pub async fn shutdown(&self) {
        if let Some(supervisor) = self.supervisor() {
            supervisor.shutdown().await;
        }
    }

    #[must_use]
    pub fn store(&self) -> Option<&DesktopStore> {
        self.supervisor()?.opened().map(Opened::store)
    }

    #[must_use]
    pub fn index(&self) -> Option<&DesktopIndex> {
        self.supervisor()?.opened()?.index()
    }

    /// The store and the index, once they are open. If the store is still opening, the call waits
    /// up to `patience` for the store to open.
    pub async fn opened_within(&self, patience: Duration) -> Option<&Opened> {
        self.supervisor()?.opened_within(patience).await
    }
}

/// The helper's executable: the `helper_path` setting, or else the file next to the running
/// executable.
#[must_use]
pub fn helper_path(settings: &Settings) -> PathBuf {
    if let Some(path) = &settings.desktop_capture.helper_path {
        return path.clone();
    }
    let mut path = std::env::current_exe().unwrap_or_else(|err| {
        warn!(?err, "could not find the running executable, so the helper is looked up in PATH");
        PathBuf::new()
    });
    path.set_file_name(HELPER_NAME);
    path.set_extension(std::env::consts::EXE_EXTENSION);
    path
}

/// Whether the desktop capture directory `dir` holds a store that an earlier capture made.
///
/// A stop alone creates the directory, to hold the stop file, so a directory that holds only the
/// stop file doesn't count as a store.
#[must_use]
pub fn store_exists(dir: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    entries.filter_map(Result::ok).any(|entry| !StopFile::is_named(&entry.file_name()))
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    /// A stop alone creates the directory, and doesn't make a store.
    #[rstest]
    #[case::missing(None, false)]
    #[case::empty(Some(&[][..]), false)]
    #[case::stop_file(Some(&["stopped"][..]), false)]
    #[case::store(Some(&["stopped", "db-1"][..]), true)]
    fn knows_whether_a_store_exists(#[case] files: Option<&[&str]>, #[case] exists: bool) {
        let home = tempfile::tempdir().expect("tempdir");
        let dir = home.path().join("desktop-capture");
        if let Some(files) = files {
            std::fs::create_dir(&dir).expect("create the directory");
            for file in files {
                std::fs::write(dir.join(file), b"").expect("write");
            }
        }

        assert_eq!(store_exists(&dir), exists);
    }
}
