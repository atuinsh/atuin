//! The Linux backend: AT-SPI over D-Bus.

mod apps;
mod bus;
mod desktop_entries;
mod events;
mod location;
mod normalize;
mod notices;
mod outline;
mod pauses;
mod private;
mod procs;
mod quirks;
mod read;
mod roles;
mod unlisted;

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use futures::{Stream, StreamExt as _, future, stream};
use zbus::{Connection, OwnedVariant};

use self::apps::{AppKind, Profiles, Waiter};
pub use self::bus::{App, Object};
use self::bus::{Bus, IN_FLIGHT, REGISTRY, STATE_SHOWING, has_state, is_active_window};
use self::events::{EventQueue, event_counts};
use self::private::PrivateWindows;
use self::procs::{Process, Sandbox, find_browsers, own_processes};
use self::read::Reader;
use self::roles::AtspiRole;
use crate::backend::{
    AppIdentity, AppInfo, Backend, Error, Event, EventCount, Inspect, Limits, Platform, ProcessApp,
    Purpose, Reading, Role, WindowClass,
};
use crate::outline::Outline;
use crate::readiness::{MissingAppNotes, Remedy, SessionVariable, UnreadyApp};
use crate::urls::is_visitable;

/// How long a running application needs to join the accessibility bus.
const WAKE_TIME: Duration = Duration::from_secs(3);

/// How many times [`AtSpi::app_name`] asks an application for its name.
const NAME_ATTEMPTS: u32 = 4;

/// How long [`AtSpi::app_name`] waits before it asks again.
const NAME_RETRY_DELAY: Duration = Duration::from_millis(150);

/// The names that terminal emulators go by on the accessibility bus.
///
/// Some terminals name themselves by their application id: GNOME Terminal 3.56 appears on the
/// bus as `org.gnome.Terminal`.
const TERMINAL_APPS: &[&str] = &[
    "Alacritty",
    "Black Box",
    "blackbox-terminal",
    "com.mitchellh.ghostty",
    "com.raggesilver.BlackBox",
    "Console",
    "contour",
    "cool-retro-term",
    "deepin-terminal",
    "foot",
    "Ghostty",
    "gnome-terminal",
    "gnome-terminal-server",
    "guake",
    "Hyper",
    "kgx",
    "kitty",
    "konsole",
    "lxterminal",
    "mate-terminal",
    "org.gnome.Console",
    "org.gnome.Ptyxis",
    "org.gnome.Terminal",
    "org.kde.konsole",
    "org.kde.yakuake",
    "org.wezfurlong.wezterm",
    "ptyxis",
    "qterminal",
    "rio",
    "st",
    "Tabby",
    "Terminal",
    "terminator",
    "terminology",
    "tilda",
    "tilix",
    "Warp",
    "WezTerm",
    "wezterm-gui",
    "xfce4-terminal",
    "xterm",
    "yakuake",
];

/// How many elements outside documents a search for the documents of a window visits.
const MAX_FRAME_ELEMENTS: usize = 2_000;

/// The session-wide `org.a11y.Status.IsEnabled` flag, as it was before this process connected.
///
/// Qt applications and Firefox join the accessibility bus only when the flag is on. The
/// registry turns the flag on when a client starts listening for events, and off when the last
/// listening client leaves. An application that was already running needs a moment to join.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToolkitSwitch {
    On,
    Off,
    Unknown,
}

/// A connection to the accessibility bus of the session.
pub struct AtSpi {
    bus: Bus,
    toolkit_switch: ToolkitSwitch,
    events: EventQueue,
    profiles: Arc<Profiles>,
    private_windows: PrivateWindows,
}

impl AtSpi {
    async fn open(purpose: Purpose) -> Result<Self, zbus::Error> {
        let session = Connection::session().await?;
        let address: String = session
            .call_method(
                Some("org.a11y.Bus"),
                "/org/a11y/bus",
                Some("org.a11y.Bus"),
                "GetAddress",
                &(),
            )
            .await?
            .body()
            .deserialize()?;
        let conn = zbus::connection::Builder::address(address.as_str()).build().await?;
        let status = session
            .call_method(
                Some("org.a11y.Bus"),
                "/org/a11y/bus",
                Some("org.freedesktop.DBus.Properties"),
                "Get",
                &("org.a11y.Status", "IsEnabled"),
            )
            .await
            .and_then(|reply| reply.body().deserialize::<OwnedVariant>())
            .and_then(bool::try_from);
        let toolkit_switch = match status {
            Ok(true) => ToolkitSwitch::On,
            Ok(false) => ToolkitSwitch::Off,
            Err(_) => ToolkitSwitch::Unknown,
        };

        let bus = Bus::new(conn);
        let registry = bus.owner_of(REGISTRY).await;
        Ok(Self {
            events: EventQueue::open(&address, purpose, registry).await?,
            bus,
            toolkit_switch,
            profiles: Arc::default(),
            private_windows: PrivateWindows::default(),
        })
    }

    /// Lists the root object of every application on the accessibility bus, except the
    /// applications that only draw the frames of other windows and the desktop shell.
    async fn roots(&self) -> Result<Vec<Object>, Error> {
        let roots = self.bus.children(&Object::root(App::new(REGISTRY))).await?;
        Ok(stream::iter(roots)
            .map(|root| async move {
                let kind = self.profiles.profile(&self.bus, root.app(), Waiter::Call).await.kind;
                (kind == AppKind::Application).then_some(root)
            })
            .buffered(IN_FLIGHT)
            .filter_map(future::ready)
            .collect()
            .await)
    }

    /// Finds the window that the user is working in.
    ///
    /// A window can keep its active state while it is minimized or hidden, so the window must
    /// also be showing. A window that looks like a popup counts only when no other window is
    /// active: the window manager xfwm4 keeps an unnamed window of the `window` role active and
    /// showing at all times.
    async fn active_among(&self, windows: impl Stream<Item = Object>) -> Option<Object> {
        let active: Vec<Object> = windows
            .map(|window| async move {
                let state = self.bus.state(&window).await.ok()?;
                is_active_window(&state).then_some(window)
            })
            .buffer_unordered(IN_FLIGHT)
            .filter_map(future::ready)
            .collect()
            .await;
        let mut popup = None;
        for window in active {
            if !matches!(self.window_class(&window).await, Ok(WindowClass::Popup)) {
                return Some(window);
            }
            popup.get_or_insert(window);
        }
        popup
    }

    /// Finds the documents of `window` that lie outside other documents, by walking the
    /// elements around them. Controls such as toolbars are left out.
    ///
    /// A Collection query for documents would be one call, but Firefox answered it for a window
    /// with a 3,000-line page in more than a second, the timeout of a call.
    async fn outermost_documents(&self, window: &Object) -> Vec<Object> {
        let mut documents = Vec::new();
        let mut visited = HashSet::from([window.clone()]);
        let mut frontier = vec![window.clone()];
        while !frontier.is_empty() && visited.len() < MAX_FRAME_ELEMENTS {
            let children: Vec<Vec<Object>> = stream::iter(std::mem::take(&mut frontier))
                .map(|object| async move { self.bus.children(&object).await.unwrap_or_default() })
                .buffered(IN_FLIGHT)
                .collect()
                .await;
            // An application can list an element below itself, and the search would never end.
            let unvisited =
                children.into_iter().flatten().filter(|child| visited.insert(child.clone()));
            let children: Vec<Child> = stream::iter(unvisited)
                .map(|object| async move {
                    Child {
                        role: self.bus.role(&object).await.ok().map(AtspiRole::role),
                        object,
                    }
                })
                .buffered(IN_FLIGHT)
                .collect()
                .await;
            for child in children {
                match child.role {
                    Some(Role::Document) => documents.push(child.object),
                    Some(role) if role.is_readable() => frontier.push(child.object),
                    // An element whose role is unknown could be a password field.
                    Some(_) | None => {}
                }
            }
        }
        documents
    }
}

/// An element that a search for documents reached.
struct Child {
    object: Object,
    /// The read role, when the element answered for its role.
    role: Option<Role>,
}

/// An application, and the process that owns the application's connection when the bus
/// reported that process.
struct Owner {
    app: App,
    pid: Option<u32>,
}

impl Backend for AtSpi {
    type App = App;
    type Window = Object;

    /// Connects and registers for events, regardless of `purpose`.
    ///
    /// The registration is what makes the registry turn toolkit accessibility on, and Qt
    /// applications and Firefox expose nothing without it.
    async fn connect(purpose: Purpose) -> Result<Self, Error> {
        Self::open(purpose).await.map_err(|error| Error::Unavailable(error.into()))
    }

    /// Reports the time that applications need before a read finds their content.
    ///
    /// Connecting turned toolkit accessibility on when it was off, and applications then join
    /// the bus. Asking every window for the document that it shows makes the applications
    /// that build their pages on request start doing so.
    async fn wake_apps(&self) -> Option<Duration> {
        let joining = (self.toolkit_switch != ToolkitSwitch::On).then_some(WAKE_TIME);
        let roots = self.roots().await.unwrap_or_default();
        let documents: usize = stream::iter(roots)
            .map(|root| async move {
                self.profiles.asks_for_relations(root.app());
                let mut documents = 0;
                for window in self.bus.children(&root).await.unwrap_or_default() {
                    documents += usize::from(self.bus.embedded_document(&window).await.is_some());
                }
                documents
            })
            .buffer_unordered(IN_FLIGHT)
            .fold(0, |total, documents| async move { total + documents })
            .await;
        joining.max((documents > 0).then_some(quirks::PAGE_BUILD_TIME))
    }

    async fn apps(&self) -> Result<Vec<App>, Error> {
        Ok(self.roots().await?.into_iter().map(Object::into_app).collect())
    }

    async fn windows(&self, app: &App) -> Result<Vec<Object>, Error> {
        self.bus.children(&Object::root(app.clone())).await
    }

    fn app_of(&self, window: &Object) -> App {
        window.app().clone()
    }

    /// Finds the application whose connection to the accessibility bus belongs to `pid`.
    ///
    /// Returns [`ProcessApp::Unknown`] when the applications can't be listed, or when an
    /// application didn't report its process and no other application runs as `pid`.
    async fn app_of_process(&self, pid: u32) -> ProcessApp<App> {
        let Ok(apps) = self.apps().await else {
            return ProcessApp::Unknown;
        };
        let mut owners = stream::iter(apps)
            .map(|app| async move {
                let owner = self.bus.connection_pid(&app).await;
                Owner { app, pid: owner }
            })
            .buffer_unordered(IN_FLIGHT);
        let mut found = ProcessApp::NoApp;
        while let Some(owner) = owners.next().await {
            match owner.pid {
                Some(owner_pid) if owner_pid == pid => return ProcessApp::App(owner.app),
                Some(_) => {}
                None => found = ProcessApp::Unknown,
            }
        }
        found
    }

    /// Finds the desktop entry of `id` in the data directories, and the running applications whose
    /// processes run in the Flatpak sandbox or the systemd unit of `id`.
    ///
    /// A Flatpak application's connection to the accessibility bus is owned by its D-Bus proxy,
    /// which runs in the application's sandbox.
    async fn identify_app_id(&self, id: &str) -> AppIdentity<App> {
        let entry_names = desktop_entries::names(id);
        let apps = self.apps().await.unwrap_or_default();
        let apps = stream::iter(apps)
            .map(|app| async move {
                let pid = self.bus.connection_pid(&app).await;
                Owner { app, pid }
            })
            .buffer_unordered(IN_FLIGHT)
            .filter_map(|owner| {
                let runs = owner.pid.is_some_and(|pid| procs::runs_app_id(pid, id));
                future::ready(runs.then_some(owner.app))
            })
            .collect()
            .await;
        AppIdentity { entry_names, apps }
    }

    fn is_terminal_name(name: &str) -> bool {
        TERMINAL_APPS.iter().any(|terminal| terminal.eq_ignore_ascii_case(name))
    }

    /// Drops content changes from hidden documents that aren't pages, such as Firefox's
    /// preloaded New Tab page.
    async fn next(&mut self) -> Option<Event<Self>> {
        self.events.next(&self.bus, &self.profiles).await
    }

    async fn active_window(&self) -> Option<Object> {
        let roots = self.roots().await.ok()?;
        let windows = stream::iter(roots)
            .map(|root| async move { self.bus.children(&root).await.unwrap_or_default() })
            .buffer_unordered(IN_FLIGHT)
            .flat_map(stream::iter);
        self.active_among(windows).await
    }

    async fn active_window_of(&self, app: &App) -> Option<Object> {
        self.active_among(stream::iter(self.windows(app).await.ok()?)).await
    }

    async fn top_level(&self, window: Object) -> Object {
        let Ok(windows) = self.windows(window.app()).await else {
            return window;
        };
        if windows.contains(&window) {
            return window;
        }
        self.active_among(stream::iter(windows)).await.unwrap_or(window)
    }

    /// A popup has the role of a window without a frame, and no name.
    ///
    /// A window with the `notification` role is a [`WindowClass::NoticeWindow`] when the session
    /// bus is monitored, because the bus names the application that sent a notification, and the
    /// window belongs to the notification service.
    async fn window_class(&self, window: &Object) -> Result<WindowClass, Error> {
        Ok(match self.bus.role(window).await? {
            AtspiRole::WINDOW | AtspiRole::MENU | AtspiRole::POPUP_MENU | AtspiRole::TOOL_TIP => {
                if self.bus.name(window).await?.is_empty() {
                    WindowClass::Popup
                } else {
                    WindowClass::Regular
                }
            }
            AtspiRole::DIALOG | AtspiRole::ALERT => WindowClass::Dialog,
            AtspiRole::NOTIFICATION if self.events.watches_notices() => WindowClass::NoticeWindow,
            AtspiRole::NOTIFICATION => WindowClass::Notification,
            _ => WindowClass::Regular,
        })
    }

    async fn window_title(&self, window: &Object) -> Result<String, Error> {
        self.bus.name(window).await
    }

    async fn app_name(&self, app: &App) -> Result<String, Error> {
        // A toolkit can announce a window before the application's root object answers.
        let root = Object::root(app.clone());
        let mut attempts_left = NAME_ATTEMPTS;
        loop {
            match self.bus.name(&root).await {
                Err(Error::Platform(_)) if attempts_left > 1 => {
                    attempts_left -= 1;
                    tokio::time::sleep(NAME_RETRY_DELAY).await;
                }
                result => return result,
            }
        }
    }

    async fn read(&self, window: &Object, limits: Limits) -> Result<Reading, Error> {
        Reader::new(&self.bus, limits).read(window, &self.profiles, &self.private_windows).await
    }

    /// Lists the hidden documents of `window` that are pages a person can visit, outside other
    /// documents. In Firefox, these are the background tabs that have loaded.
    async fn hidden_documents(&self, window: &Object) -> Vec<Object> {
        stream::iter(self.outermost_documents(window).await)
            .map(|document| async move {
                let (state, url) =
                    future::join(self.bus.state(&document), self.bus.document_url(&document)).await;
                let hidden = state.is_ok_and(|state| !has_state(&state, STATE_SHOWING));
                let page = url.is_some_and(|url| is_visitable(&url));
                (hidden && page).then_some(document)
            })
            .buffered(IN_FLIGHT)
            .filter_map(future::ready)
            .collect()
            .await
    }

    fn take_event_counts(&mut self) -> impl Iterator<Item = EventCount<App>> + use<> {
        self.events
            .take_tally()
            .into_iter()
            .flat_map(|(bus, tally)| event_counts(App::new(&bus), tally))
    }
}

impl Platform for AtSpi {
    /// The accessibility bus comes with at-spi2-core, and a session starts it at login.
    fn service_instructions() -> Option<&'static str> {
        Some("Install at-spi2-core, and log in again.")
    }

    fn missing_app_notes() -> MissingAppNotes {
        quirks::MISSING_APP_NOTES
    }

    fn session_variables() -> &'static [SessionVariable] {
        &quirks::SESSION_VARIABLES
    }
}

impl Inspect for AtSpi {
    /// Finds the browsers and Electron applications that stayed off the accessibility bus.
    ///
    /// Chromium decides at its start whether to join, and Firefox sometimes misses the moment
    /// at which toolkit accessibility turns on.
    async fn unready_apps(&self) -> Result<Vec<UnreadyApp>, Error> {
        let on_bus: Vec<u32> = stream::iter(self.roots().await?)
            .map(|root| async move { self.bus.connection_pid(root.app()).await })
            .buffer_unordered(IN_FLIGHT)
            .filter_map(future::ready)
            .collect()
            .await;
        let processes: Vec<Process> = own_processes().collect();
        // A sandboxed application is on the bus through its D-Bus proxy, a process of its own
        // in the same Flatpak instance.
        let instances_on_bus: Vec<u64> =
            on_bus.iter().filter_map(|&pid| procs::instance_of(&processes, pid)).collect();
        let unready = find_browsers(&processes)
            .into_iter()
            .filter(|browser| !on_bus.contains(&browser.pid))
            .filter(|browser| match browser.sandbox {
                Sandbox::Flatpak {
                    instance: Some(instance),
                } => !instances_on_bus.contains(&instance),
                Sandbox::Flatpak { instance: None } | Sandbox::None => true,
            })
            .map(|browser| UnreadyApp {
                name: browser.name,
                pid: browser.pid,
                remedy: match browser.sandbox {
                    Sandbox::None => Remedy::Restart,
                    Sandbox::Flatpak { .. } => Remedy::RestartWithVariable,
                },
            });
        Ok(unready.collect())
    }

    fn shows_only_prompts(&self, _: &App, name: &str) -> bool {
        quirks::shows_only_prompts(name)
    }

    async fn app_info(&self, app: &App) -> Result<AppInfo, Error> {
        let (name, profile, pid) = future::join3(
            self.app_name(app),
            self.profiles.profile(&self.bus, app, Waiter::Call),
            self.bus.connection_pid(app),
        )
        .await;
        Ok(AppInfo {
            name: name?,
            toolkit: profile.toolkit.named().map(|toolkit| toolkit.name.clone()),
            pid,
        })
    }

    async fn outline(&self, window: &Object, max_entries: usize) -> Result<Outline, Error> {
        outline::outline(&self.bus, window, max_entries).await
    }
}
