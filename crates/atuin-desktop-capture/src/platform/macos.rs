//! The macOS backend: the AX API of `ApplicationServices`.

// - `element`: retained AX elements, and the values of their attributes.
// - `desktop`: the applications and windows on the desktop.
// - `dialogs`: which windows, and which groups inside a window, are dialogs.
// - `private`: whether a window belongs to a browser's private browsing mode.
// - `reader`: reads a window into a tree, or into an outline.
// - `watcher`: watches the desktop from the main thread, and sends the events.
// - `observer`: the observers, subscriptions, and timer that the watcher keeps on the run loop.
// - `owners`: the applications that own windows, which the watcher observes.
// - `wakes`: when each application was asked to build its tree.
// - `banners`: reads the notification banners that Notification Center draws.
// - `main_thread`: runs the main thread's run loop, where the watcher lives.
// - `permission`: whether macOS trusts this process to use the AX API, and for how long.
// - `quirks`: what the backend does differently for one application.
// - `ffi`: the C functions of the AX API and CoreGraphics.

mod banners;
mod desktop;
mod dialogs;
mod element;
mod ffi;
mod main_thread;
mod observer;
mod owners;
mod permission;
mod private;
mod quirks;
mod reader;
mod wakes;
mod watcher;

use std::sync::Arc;
use std::time::Duration;

use atuin_common::os::macos::responsibility;
use atuin_common::os::process::current_exe_name;
use objc2_app_kit::NSRunningApplication;
use tokio::sync::mpsc;

use self::desktop::{
    focused_window, is_terminal, is_terminal_name, readable_apps, screen_now, set_messaging_timeout,
};
use self::dialogs::DialogProbe;
use self::element::Element;
pub use self::element::Pid;
use self::permission::{Prompt, close_permission_alert, is_trusted};
use self::reader::Reader;
use self::watcher::{Shared, Watcher, WatcherHandle};
use crate::backend::{
    Access, AppInfo, Backend, Error, Event, EventCount, Inspect, Limits, Loading, Platform,
    PrivateWindows, ProcessApp, Purpose, Reading, ResponsibilityError, Screen, WindowClass,
    WindowKind,
};
use crate::outline::Outline;
use crate::readiness::{AccessLifetime, UnreadyApp};

/// A window of an application.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Window {
    element: Element,
    pid: Pid,
}

impl Window {
    const fn new(element: Element, pid: Pid) -> Self {
        Self { element, pid }
    }
}

/// A connection to the AX API.
///
/// A connection for capture runs a watcher on the main thread. While capture is running, the
/// watcher follows the frontmost application and the applications that own windows, and sends
/// their events.
pub struct MacAx {
    events: mpsc::UnboundedReceiver<Event<Self>>,
    shared: Arc<Shared>,
    watcher: Option<WatcherHandle>,
    dialogs: DialogProbe,
    private_windows: private::PrivateWindows,
}

#[expect(
    clippy::unused_async_trait_impl,
    reason = "the trait methods are async, and AX calls block instead of awaiting"
)]
impl Backend for MacAx {
    type App = Pid;
    type Window = Window;

    /// Connects, and starts a paused watcher of the desktop when `purpose` is capture.
    async fn connect(purpose: Purpose) -> Result<Self, Error> {
        if Self::access() == Access::Denied {
            return Err(Error::AccessDenied);
        }
        // The AX API reads only the session at the screen, so a user elsewhere doesn't have a
        // desktop to read. For example, a user who is logged in over SSH is elsewhere.
        if !atuin_common::os::macos::console::is_at_screen() {
            return Err(Error::Unavailable(Box::new(NotAtScreen)));
        }
        if purpose == Purpose::Capture && !main_thread::is_served() {
            return Err(Error::MainThreadNotServed);
        }
        set_messaging_timeout();
        let (sender, receiver) = mpsc::unbounded_channel();
        let shared = Arc::new(Shared::default());
        // Without a watcher, the sender drops here, and the events end at once.
        let watcher =
            (purpose == Purpose::Capture).then(|| Watcher::spawn(Arc::clone(&shared), sender));
        Ok(Self {
            events: receiver,
            shared,
            watcher,
            dialogs: DialogProbe::default(),
            private_windows: private::PrivateWindows::default(),
        })
    }

    /// Asks every application whose windows capture may read to build its tree. Chromium and
    /// Electron build their trees only on request.
    async fn wake_apps(&self) -> Option<Duration> {
        self.shared.forget_exited();
        for app in readable_apps(self.shared.bundles()) {
            self.shared.wake(app);
        }
        Some(quirks::WAKE_TIME)
    }

    /// Lists the applications that own an on-screen window or modal panel, frontmost first.
    async fn apps(&self) -> Result<Vec<Pid>, Error> {
        Ok(readable_apps(self.shared.bundles()))
    }

    async fn windows(&self, app: &Pid) -> Result<Vec<Window>, Error> {
        let windows = Element::application(*app).elements("AXWindows");
        Ok(windows.map(|element| Window::new(element, *app)).collect())
    }

    fn app_of(&self, window: &Window) -> Pid {
        window.pid
    }

    /// The running application of the process `pid`. The notice of a banner names the process
    /// of the application whose name the banner shows.
    async fn app_of_process(&self, pid: u32) -> ProcessApp<Pid> {
        let Ok(pid) = i32::try_from(pid) else {
            return ProcessApp::NoApp;
        };
        match NSRunningApplication::runningApplicationWithProcessIdentifier(pid) {
            Some(_) => ProcessApp::App(Pid::new(pid)),
            None => ProcessApp::NoApp,
        }
    }

    fn is_terminal_name(name: &str) -> bool {
        is_terminal_name(name)
    }

    fn is_terminal(&self, app: &Pid, name: &str) -> bool {
        is_terminal(*app, name, self.shared.bundles())
    }

    fn is_left_out(&self, app: &Pid) -> bool {
        self.shared.bundles().matches(*app, |bundle_id| {
            quirks::is_permission_alert(bundle_id) || quirks::is_login_window(bundle_id)
        })
    }

    async fn next(&mut self) -> Option<Event<Self>> {
        self.events.recv().await
    }

    /// Stops the watcher from reading banners, titles, and content, and from waking
    /// applications.
    fn pause(&self) {
        self.shared.pause();
        if let Some(watcher) = &self.watcher {
            watcher.pause();
        }
    }

    fn resume(&self) {
        self.shared.resume();
        if let Some(watcher) = &self.watcher {
            watcher.resume();
        }
    }

    async fn active_window(&self) -> Option<Window> {
        self.shared.active_window()
    }

    async fn active_window_of(&self, app: &Pid) -> Option<Window> {
        let element = focused_window(*app)?;
        Some(Window::new(element, *app))
    }

    /// Returns the window that a sheet is attached to, and any other window as it is.
    ///
    /// A sheet takes the focus from its window, and the user still works in that window.
    async fn top_level(&self, window: Window) -> Window {
        if !window.element.has("AXRole", "AXSheet") {
            return window;
        }
        match window.element.element("AXParent") {
            Some(parent) if parent.has("AXRole", "AXWindow") => Window::new(parent, window.pid),
            _ => window,
        }
    }

    /// Tells dialogs from regular windows. A menu never becomes the window that the watcher
    /// follows, so every other window is [`WindowClass::Regular`].
    ///
    /// The watcher follows the focused window of the frontmost application. macOS reports
    /// notifications as banners, which capture reads as notices.
    async fn window_class(&self, window: &Window) -> Result<WindowClass, Error> {
        if self.dialogs.is_dialog(&window.element, window.pid, self.shared.bundles()) {
            Ok(WindowClass::Dialog)
        } else {
            Ok(WindowClass::Regular)
        }
    }

    async fn window_title(&self, window: &Window) -> Result<String, Error> {
        // A dialog inside a window doesn't have a title, and its value is its message. For
        // example, a page's alert in Safari is such a dialog.
        let title = window
            .element
            .string("AXTitle")
            .or_else(|| window.element.string("AXValue"))
            .ok_or(Error::NoAnswer)?;
        let placeholder = self
            .shared
            .bundles()
            .matches(window.pid, |bundle_id| quirks::is_placeholder_title(bundle_id, &title));
        Ok(if placeholder {
            String::new()
        } else {
            title
        })
    }

    async fn app_name(&self, app: &Pid) -> Result<String, Error> {
        Element::application(*app).string("AXTitle").ok_or(Error::NoAnswer)
    }

    /// Fails when the screen is locked. The watcher can learn of a lock a moment after the
    /// lock screen shows, and a read in that moment could otherwise find the lock screen.
    async fn read(&self, window: &Window, limits: Limits) -> Result<Reading, Error> {
        if screen_now() == Screen::Locked {
            return Err(Error::Unavailable(Box::new(ScreenIsLocked)));
        }
        if limits.private_windows == PrivateWindows::Exclude {
            let bundle_id = self.shared.bundles().get(window.pid);
            if self.private_windows.is_private(&window.element, window.pid, bundle_id)? {
                return Ok(Reading::of_private_window());
            }
        }
        let read = Reader::read_window(&window.element, limits).ok_or(Error::NoAnswer)?;
        // An editor reports its file here. A browser reports the page on its web area instead.
        let url = window.element.string("AXDocument").or(read.url);
        // An application can show its window before the document in it, for a moment after it
        // was first asked to build its tree. Safari on macOS 27.2 showed an empty browser view,
        // added the page about a second later, and announced nothing.
        let loading = if read.documents == 0 && self.shared.may_be_building(window.pid) {
            Loading::InProgress
        } else {
            read.loading
        };
        Ok(Reading {
            root: read.root,
            // Terminals are known by their applications on this platform. See `is_terminal`.
            kind: WindowKind::Other,
            loading,
            url,
            stats: read.stats,
        })
    }

    /// Returns an empty iterator. Only a connection for capture receives events, and nothing asks a
    /// capture for its counts.
    fn take_event_counts(&mut self) -> impl Iterator<Item = EventCount<Pid>> + use<> {
        std::iter::empty()
    }
}
impl Platform for MacAx {
    fn access() -> Access {
        if is_trusted(Prompt::Skip) {
            Access::Granted
        } else {
            Access::Denied
        }
    }

    fn request_access() {
        let _ = is_trusted(Prompt::Show);
    }

    fn open_access_settings() {
        permission::open_settings();
    }

    fn access_instructions() -> Option<&'static str> {
        Some("Enable it in System Settings → Privacy & Security → Accessibility.")
    }

    /// The alert names this program by its file name.
    fn close_access_request() {
        if let Some(program) = current_exe_name() {
            close_permission_alert(&program);
        }
    }

    /// Runs the rest of the program in a child process that is responsible for itself, so
    /// that macOS charges the permission to this program.
    fn take_responsibility() -> Result<(), ResponsibilityError> {
        responsibility::respawn_disclaimed("ATUIN_DESKTOP_CAPTURE_DISCLAIMED")
            .map_err(ResponsibilityError::new)
    }

    /// Runs `work` while the main thread runs its run loop, which is the only one where
    /// `NSWorkspace` announces which application is active.
    fn serve_main_thread<R: Send>(work: impl FnOnce() -> R + Send) -> R {
        main_thread::serve(work)
    }

    /// Reads the lifetime from the code signature, which macOS ties the permission to.
    fn access_lifetime() -> AccessLifetime {
        permission::access_lifetime()
    }
}

#[expect(
    clippy::unused_async_trait_impl,
    reason = "the trait methods are async, and AX calls block instead of awaiting"
)]
impl Inspect for MacAx {
    /// Returns an empty list, because Chrome, Safari, and Electron applications don't need
    /// switches on macOS.
    async fn unready_apps(&self) -> Result<Vec<UnreadyApp>, Error> {
        Ok(Vec::new())
    }

    fn shows_only_prompts(&self, app: &Pid, name: &str) -> bool {
        self.shared.bundles().matches(*app, quirks::shows_only_prompts)
            || quirks::is_passphrase_prompt(name)
    }

    async fn app_info(&self, app: &Pid) -> Result<AppInfo, Error> {
        Ok(AppInfo {
            name: self.app_name(app).await?,
            toolkit: None,
            pid: u32::try_from(app.raw()).ok(),
        })
    }

    async fn outline(&self, window: &Window, max_entries: usize) -> Result<Outline, Error> {
        Reader::outline(&window.element, max_entries).ok_or(Error::NoAnswer)
    }
}

/// The reason that a user who isn't logged in at the Mac's screen doesn't have a desktop to
/// read.
#[derive(Debug, thiserror::Error)]
#[error("this user isn't logged in at the Mac's screen")]
struct NotAtScreen;

/// The reason that a read fails when the screen is locked.
#[derive(Debug, thiserror::Error)]
#[error("the screen is locked")]
struct ScreenIsLocked;
