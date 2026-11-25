//! What every part of capture shares: the backend, the configuration, the names and ids that
//! capture has handed out, and the records that wait to be handed out.

use std::collections::VecDeque;
use std::time::Duration;

use atuin_common::collections::ForgetfulMap;
use tokio::time::Instant;
use tracing::debug;

use super::config::{Config, Scope};
use super::record::{Record, WindowId};
use super::scrubber::Scrubber;
use crate::backend::{Backend, Error, Limits, ProcessApp, Reading};

/// How many windows capture remembers the ids of.
pub const MAX_KNOWN_WINDOWS: usize = 4096;

/// How many applications capture remembers the names of.
const MAX_KNOWN_APPS: usize = 1024;

/// How many processes that sent notifications capture remembers the applications of.
const MAX_KNOWN_SENDERS: usize = 256;

pub struct Context<B: Backend> {
    pub backend: B,
    pub config: Config,
    pub scrubber: Scrubber,
    pub windows: WindowRegistry<B::Window>,
    /// The records that wait to be handed out.
    pub records: VecDeque<Record>,
    /// The applications that capture has named, and whether capture may read them.
    ///
    /// A handle can stand for another application later, such as a process id that the system
    /// reuses, so an application that joins is named again.
    apps: ForgetfulMap<B::App, KnownApp>,
    /// The application that runs as each process that sent a notification, or `None` when no
    /// application with an accessibility tree does.
    senders: ForgetfulMap<u32, Option<B::App>>,
}

/// What capture learned about an application.
struct KnownApp {
    name: String,
    allowed: Allowed,
}

/// Whether capture may read the windows of an application.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Allowed {
    Yes,
    No,
}

/// A value with the time that it took to get.
pub struct Timed<T> {
    pub value: T,
    pub elapsed: Duration,
}

impl<B: Backend> Context<B> {
    pub fn new(backend: B, config: Config, next_window: u64) -> Self {
        Self {
            backend,
            config,
            scrubber: Scrubber::default(),
            windows: WindowRegistry::new(next_window),
            records: VecDeque::new(),
            apps: ForgetfulMap::new(MAX_KNOWN_APPS),
            senders: ForgetfulMap::new(MAX_KNOWN_SENDERS),
        }
    }

    /// The name of `app`.
    pub async fn app_name(&mut self, app: &B::App) -> Result<&str, Error> {
        Ok(&self.known_app(app).await?.name)
    }

    /// Returns the name of `app` when capture may read the windows of `app`.
    pub async fn allowed_app_name(&mut self, app: &B::App) -> Option<&str> {
        match self.known_app(app).await {
            Ok(KnownApp {
                name,
                allowed: Allowed::Yes,
            }) => Some(name),
            Ok(_) => None,
            Err(error) => {
                debug!(?app, %error, "skipping an application without a name");
                None
            }
        }
    }

    /// Whether capture may read the windows of `app`.
    ///
    /// Capture reads an application unless the backend knows the application as a terminal
    /// or leaves the application out, or unless the filter leaves the application out.
    pub async fn allows_app(&mut self, app: &B::App) -> bool {
        self.allowed_app_name(app).await.is_some()
    }

    /// Whether capture may read an application named `name`. Terminals and the applications
    /// that the filter leaves out are not read.
    pub fn allows_name(&self, name: &str) -> bool {
        self.allows_app_named([name])
    }

    /// Whether capture may read an application that goes by every one of `names`. None of the
    /// names may be a terminal's, and the filter must allow an application of these names.
    pub fn allows_app_named<'a, I>(&self, names: I) -> bool
    where
        I: IntoIterator<Item = &'a str>,
        I::IntoIter: Clone,
    {
        let names = names.into_iter();
        !names.clone().any(B::is_terminal_name) && self.config.filter.allows_app_named(names)
    }

    /// Whether capture may keep a notification that the process `pid` sent.
    ///
    /// A process that runs an application with an accessibility tree passes when capture may
    /// read that application. Any other process passes, because only the name in the
    /// notification can tell which application sent the notification. For example, a
    /// Flatpak application sends notifications through its D-Bus proxy, which has no
    /// accessibility tree. A process whose lookup failed passes only when the filter starts
    /// from every application.
    pub async fn allows_sender(&mut self, pid: u32) -> bool {
        match self.app_of_sender(pid).await {
            ProcessApp::App(app) => self.allows_app(&app).await,
            ProcessApp::NoApp => true,
            ProcessApp::Unknown => self.config.filter.scope == Scope::All,
        }
    }

    /// The application that runs as the process `pid`, which sent a notification.
    ///
    /// Capture remembers each answer except a failed lookup, until [`Self::forget_app`].
    pub async fn app_of_sender(&mut self, pid: u32) -> ProcessApp<B::App> {
        if let Some(app) = self.senders.get(&pid) {
            return app.clone().map_or(ProcessApp::NoApp, ProcessApp::App);
        }
        let app = self.backend.app_of_process(pid).await;
        let known = match &app {
            ProcessApp::App(app) => Some(app.clone()),
            ProcessApp::NoApp => None,
            ProcessApp::Unknown => {
                debug!(pid, "the application behind a process is unknown");
                return app;
            }
        };
        self.senders.insert(pid, known);
        app
    }

    /// Makes capture name `app` again, because its handle may stand for a new application.
    ///
    /// Capture also forgets which applications the senders of notifications run, because a
    /// process that ran none may run the new one.
    pub fn forget_app(&mut self, app: &B::App) {
        self.apps.remove(app);
        self.senders.clear();
    }

    /// What capture knows about `app`, which `known_app` asks the backend for the first time.
    ///
    /// The filter applies to the name that the backend reports, which is the name that users
    /// see in the list of applications. Capture keeps the name cleaned.
    async fn known_app(&mut self, app: &B::App) -> Result<&KnownApp, Error> {
        if !self.apps.contains_key(app) {
            let raw_name = self.backend.app_name(app).await?;
            let allowed = if self.backend.is_terminal(app, &raw_name)
                || self.backend.is_left_out(app)
                || !self.allows_name(&raw_name)
            {
                debug!(app = raw_name, "filtering an application");
                Allowed::No
            } else {
                Allowed::Yes
            };
            let name = self.scrubber.app_name(raw_name);
            self.apps.insert(app.clone(), KnownApp { name, allowed });
        }
        Ok(self.apps.get(app).expect("the application is known"))
    }

    /// Reads `window` within the configured limits.
    pub async fn read(&self, window: &B::Window) -> Timed<Result<Reading, Error>> {
        self.read_within(window, self.config.limits).await
    }

    /// Reads `window` within `limits`.
    pub async fn read_within(
        &self,
        window: &B::Window,
        limits: Limits,
    ) -> Timed<Result<Reading, Error>> {
        let limits = Limits {
            private_windows: self.config.private_windows,
            ..limits
        };
        let started = Instant::now();
        let value = self.backend.read(window, limits).await;
        Timed {
            value,
            elapsed: started.elapsed(),
        }
    }

    /// The title of `window`, cleaned. Empty when the backend has none.
    pub async fn title(&self, window: &B::Window) -> String {
        self.scrubber.title(self.backend.window_title(window).await.unwrap_or_default())
    }
}

/// The ids of the windows that capture has handed out.
pub struct WindowRegistry<W> {
    /// Handles of closed windows are never reported as gone, so a returning window can lose
    /// its id once to bound the memory.
    known: ForgetfulMap<W, KnownWindow>,
    next: u64,
}

struct KnownWindow {
    id: WindowId,
    /// Whether the window was handed out as a dialog or a notification.
    was_handed_out: bool,
}

impl<W: Clone + Eq + std::hash::Hash> WindowRegistry<W> {
    fn new(next: u64) -> Self {
        Self {
            known: ForgetfulMap::new(MAX_KNOWN_WINDOWS),
            next,
        }
    }

    /// The id that the next new window gets.
    pub const fn next(&self) -> u64 {
        self.next
    }

    /// Returns the id of `window`, which is a new one when the window has none yet.
    pub fn id(&mut self, window: &W) -> WindowId {
        self.entry(window).id
    }

    /// Returns the id of `window`, and remembers that it was handed out as a dialog or a
    /// notification.
    pub fn handed_out_id(&mut self, window: &W) -> WindowId {
        let known = self.entry(window);
        known.was_handed_out = true;
        known.id
    }

    pub fn was_handed_out(&self, window: &W) -> bool {
        self.known.get(window).is_some_and(|known| known.was_handed_out)
    }

    fn entry(&mut self, window: &W) -> &mut KnownWindow {
        if !self.known.contains_key(window) {
            let id = WindowId(self.next);
            self.next += 1;
            self.known.insert(window.clone(), KnownWindow {
                id,
                was_handed_out: false,
            });
        }
        self.known.get_mut(window).expect("the window is known")
    }
}
