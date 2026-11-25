//! The platform-neutral interface to a desktop's accessibility API.
//!
//! A [`Backend`] lists applications and windows, reads the content of a window as a tree of
//! [`Node`]s, and reports the window events that matter to capture.

use std::borrow::Cow;
use std::fmt::Debug;
use std::hash::Hash;
use std::time::Duration;

use crate::outline::Outline;
use crate::readiness::{AccessLifetime, MissingAppNotes, SessionVariable, UnreadyApp};

/// What an element is, reduced to the distinctions that capture and rendering need.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub enum Role {
    Window,
    Document,
    Heading,
    Paragraph,
    ListItem,
    /// A row of a table or a list with columns. Its cells are its children.
    Row,
    /// The heading of a column of a table. The headings of a table are the cells of its first
    /// row.
    ColumnHeader,
    Cell,
    /// A short, named piece of UI that describes content, such as a label or a tab title.
    Label,
    /// A run of text inside a block.
    InlineText,
    Link,
    Editable,
    /// A field whose content must never be read, such as a password entry.
    Secret,
    /// A terminal, inside a window or filling it. A terminal shows commands, their output, and
    /// the secrets in both, so its content is never read. Capture keeps the element itself, to
    /// show where the terminal is.
    Terminal,
    /// Application controls that don't carry content: menus, toolbars, buttons, scroll bars.
    Controls,
    #[default]
    Container,
}

impl Role {
    /// Whether the reader should fetch this element's content and descend into its children.
    #[must_use]
    pub const fn is_readable(self) -> bool {
        !matches!(self, Self::Secret | Self::Terminal | Self::Controls)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Visibility {
    #[default]
    Showing,
    Hidden,
}

/// The state of an option such as a check box.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Mark {
    /// The element is not an option.
    #[default]
    None,
    Checked,
    Unchecked,
}

/// Whether a field holds its default value, which the serialized tree leaves out.
#[cfg(feature = "serde")]
fn is_default<T: Default + PartialEq>(value: &T) -> bool {
    *value == T::default()
}

/// One element of a window's accessibility tree.
///
/// More fields will follow, so build a node from `Node::default()`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(default))]
#[non_exhaustive]
pub struct Node {
    pub role: Role,
    /// The role in the platform's own terms, such as `push button` or `AXButton`. Empty when
    /// the platform did not tell.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "is_default"))]
    pub native_role: Cow<'static, str>,
    /// The URL of a document. Capture reduces it to its scheme, host, and path.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "is_default"))]
    pub url: Option<String>,
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "is_default"))]
    pub visibility: Visibility,
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "is_default"))]
    pub name: String,
    /// The element's own content as text. An element that has both text and children shows
    /// its text in place of the children.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "is_default"))]
    pub text: Option<String>,
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "is_default"))]
    pub heading_level: Option<u8>,
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "is_default"))]
    pub mark: Mark,
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "is_default"))]
    pub children: Vec<Self>,
}

impl Node {
    /// Whether the element can show in rendered text: it is showing, and its role is readable.
    #[must_use]
    pub fn is_rendered(&self) -> bool {
        self.visibility == Visibility::Showing && self.role.is_readable()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "link", derive(serde::Serialize, serde::Deserialize))]
pub enum Completeness {
    Complete,
    /// The result stops at a limit of size.
    Truncated,
    /// The read stops where its time ran out. An application that is busy answers slowly, and
    /// the same read can finish later.
    OutOfTime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "link", derive(serde::Serialize, serde::Deserialize))]
pub struct ReadStats {
    /// Elements that the reader queried, including hidden ones.
    pub nodes: usize,
    pub completeness: Completeness,
    /// Elements that did not tell their role, which the read leaves out with everything below
    /// them.
    #[cfg_attr(feature = "link", serde(default))]
    pub unknown_roles: usize,
}

/// Whether the documents of a window have finished loading.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Loading {
    #[default]
    Finished,
    /// A showing document reports that it is still loading, so its content is partial.
    InProgress,
}

/// What a window is, as far as capture treats windows differently.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WindowKind {
    #[default]
    Other,
    /// Most of the window is a terminal. Its content is shell history, which Atuin already
    /// captures, and its title usually names the running command.
    Terminal,
    /// The window is part of a browser's own interface, such as the list of suggestions
    /// below the address bar. It doesn't show a page.
    BrowserUi,
    /// The window belongs to a browser's private browsing mode, such as an incognito window.
    ///
    /// A backend reports this kind only for a window that carries a mark of a private window.
    /// A read whose check of the window's privacy can't finish fails instead.
    Private,
}

/// What a window is to the desktop, as far as capture follows and records windows
/// differently.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WindowClass {
    /// A window that capture follows while it is active.
    #[default]
    Regular,
    /// A short-lived surface of another window, such as a menu or the list of a combo box.
    ///
    /// A popup takes the active state from the window that opened it. Capture keeps following
    /// that window.
    Popup,
    /// A dialog or an alert: a window whose message the user can dismiss before reading it.
    Dialog,
    /// A notification that the platform reports only as a window.
    ///
    /// Capture records it like a dialog, as a notification.
    Notification,
    /// The window of a notification that also arrives as a [`Notice`]. Capture records the
    /// notice and leaves the window alone.
    NoticeWindow,
}

/// The visible content of one window.
#[derive(Debug)]
pub struct Reading {
    pub root: Node,
    pub kind: WindowKind,
    pub loading: Loading,
    /// The URL of the first visible document in the window.
    pub url: Option<String>,
    pub stats: ReadStats,
}

impl Reading {
    /// The reading of a private window, which holds only the window's kind.
    #[must_use]
    pub fn of_private_window() -> Self {
        Self {
            root: Node {
                role: Role::Window,
                ..Node::default()
            },
            kind: WindowKind::Private,
            loading: Loading::Finished,
            url: None,
            stats: ReadStats {
                nodes: 0,
                completeness: Completeness::Complete,
                unknown_roles: 0,
            },
        }
    }
}

/// How much of a document a read covers. Everything outside documents is read as shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Extent {
    /// Only the part of a document that is scrolled into view.
    Visible,
    /// The whole document of the active tab, including the part that is scrolled out of view.
    #[default]
    Document,
}

/// Whether capture records the windows of a browser's private browsing mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "link", derive(serde::Serialize, serde::Deserialize))]
pub enum PrivateWindows {
    /// A read of a private window holds only the kind [`WindowKind::Private`], and the read
    /// fails when the check of the window's privacy can't finish.
    #[default]
    Exclude,
    /// A private window reads like every other window, with the kind [`WindowKind::Other`].
    Capture,
}

/// The bounds of one read.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// The read stops after this many elements and reports [`Completeness::Truncated`].
    pub max_nodes: usize,
    /// The read stops after this much time and reports [`Completeness::OutOfTime`].
    pub budget: Duration,
    pub extent: Extent,
    /// Capture sets this field for each read from its setting of private windows.
    pub private_windows: PrivateWindows,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_nodes: 20_000,
            budget: Duration::from_millis(1500),
            extent: Extent::default(),
            private_windows: PrivateWindows::default(),
        }
    }
}

/// Why the content of a window may have changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cause {
    Loaded,
    Edited,
    Restructured,
}

/// A notification that an application sent to the desktop's notification service.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Notice {
    /// The name of the application that sent the notification. Empty when the platform doesn't
    /// name a sender.
    pub app: String,
    /// The id of the process that sent the notification, when the platform tells.
    ///
    /// The name in `app` is the one that the sender chose. The process can tell which
    /// application sent the notification when that name is not the application's own.
    pub pid: Option<u32>,
    /// The id of the application's desktop entry, such as `org.gnome.Fractal`, when the
    /// notification names one besides `app`. Empty otherwise.
    ///
    /// The sender chooses this id too. Capture checks the filter against the names that the
    /// id stands for.
    pub app_id: String,
    pub title: String,
    /// The text below the title, without markup.
    pub body: String,
}

/// A window event that matters to capture.
pub enum Event<B: Backend> {
    Activated {
        window: B::Window,
    },
    Deactivated {
        window: B::Window,
    },
    TitleChanged {
        window: B::Window,
        title: String,
    },
    /// Keyboard focus moved to an element of `app`. Some toolkits send only this event when
    /// their first window opens.
    FocusMoved {
        app: B::App,
    },
    /// A top-level window appeared. It may never become active.
    Opened {
        window: B::Window,
    },
    /// An application appeared, together with the windows that it already has. A program that
    /// only shows one dialog appears this way.
    Joined {
        app: B::App,
    },
    ContentChanged {
        app: B::App,
        /// The top-level window whose content changed. `None` when the platform could not
        /// tell, and then the change can be in any window of `app`.
        window: Option<B::Window>,
        cause: Cause,
    },
    /// An application sent a notification. The notification may disappear by itself.
    Notified {
        notice: Notice,
    },
    /// The screen locked or unlocked. A backend reports the state of the screen when it starts
    /// to send events, and again whenever the state changes.
    ///
    /// Capture reads nothing while the screen is locked. A backend that never reports the
    /// state leaves capture as if the screen were unlocked.
    ScreenChanged {
        screen: Screen,
    },
}

/// Whether the screen shows the lock screen, which hides the user's windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Locked,
    Unlocked,
}

/// How often one kind of platform event arrived from one application.
///
/// The counts include the events that [`Backend::next`] filtered out, so they show how noisy
/// an application is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventCount<A> {
    pub app: A,
    /// The platform's event, such as `object:state-changed`.
    pub event: &'static str,
    /// The platform's qualifier of the event, such as `focused`. Empty when there is none.
    pub detail: String,
    pub count: u64,
}

/// What a lookup of the application behind a process found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProcessApp<A> {
    /// The process runs this application.
    App(A),
    /// No application with an accessibility tree runs as the process.
    NoApp,
    /// The lookup failed, for example because an application did not answer in time. A later
    /// lookup can find the application.
    Unknown,
}

/// What the platform knows about the application of an app id, such as the id of a Flatpak
/// application or of a desktop entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppIdentity<A> {
    /// The names in the application's desktop entry: its name, and then its translations.
    pub entry_names: Vec<String>,
    /// The running applications that belong to the id.
    pub apps: Vec<A>,
}

impl<A> Default for AppIdentity<A> {
    fn default() -> Self {
        Self {
            entry_names: Vec::new(),
            apps: Vec::new(),
        }
    }
}

/// What an application reports about itself.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AppInfo {
    pub name: String,
    /// The UI toolkit, on platforms that report one.
    pub toolkit: Option<String>,
    pub pid: Option<u32>,
}

/// Whether this process may use the accessibility API.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    Granted,
    Denied,
}

/// What a connection is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    /// Follow window events and read windows.
    Capture,
    /// List and read windows. [`Backend::next`] yields nothing useful.
    Inspect,
}

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The user has not allowed this process to use the accessibility API.
    #[error("this process lacks the permission to use the accessibility API")]
    AccessDenied,
    #[error("this platform has no accessibility backend")]
    Unsupported,
    /// The accessibility service of the desktop could not be reached.
    #[error("the accessibility service is unavailable")]
    Unavailable(#[source] Box<dyn std::error::Error + Send + Sync>),
    /// The application or element is gone, or it took too long to answer.
    #[error("the application did not answer")]
    NoAnswer,
    /// Capture needs [`Platform::serve_main_thread`] on this platform.
    #[error("capture needs the main thread to serve the event loop")]
    MainThreadNotServed,
    #[error(transparent)]
    Platform(Box<dyn std::error::Error + Send + Sync>),
}

/// Error returned by [`Platform::take_responsibility`].
#[derive(Debug, thiserror::Error)]
#[error(transparent)]
pub struct ResponsibilityError(Box<dyn std::error::Error + Send + Sync>);

impl ResponsibilityError {
    pub fn new(error: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self(Box::new(error))
    }
}

/// What the accessibility API of a platform asks of the whole process: its permission, and
/// the thread that serves the platform's event loop.
pub trait Platform {
    /// Reports whether this process may use the accessibility API.
    #[must_use]
    fn access() -> Access {
        Access::Granted
    }

    /// Asks the platform to show its permission dialog for this process, when it has one.
    fn request_access() {}

    /// Opens the system settings in which the user grants the permission, on platforms that
    /// have such a place.
    fn open_access_settings() {}

    /// Tells the user where to grant the permission, in a sentence. `None` on platforms whose
    /// processes always have the permission.
    #[must_use]
    fn access_instructions() -> Option<&'static str> {
        None
    }

    /// Tells the user what makes the accessibility service available, in a sentence, for when
    /// [`Backend::connect`] fails with [`Error::Unavailable`].
    ///
    /// `None` on platforms whose service is always there.
    #[must_use]
    fn service_instructions() -> Option<&'static str> {
        None
    }

    /// Tells why running applications can be missing from what the accessibility API lists.
    #[must_use]
    fn missing_app_notes() -> MissingAppNotes {
        MissingAppNotes::default()
    }

    /// Closes the permission dialog that [`request_access`](Self::request_access) opened.
    ///
    /// Call this once this process has the permission, because some platforms leave the
    /// dialog open after the user grants the permission.
    fn close_access_request() {}

    /// Makes this program, instead of the program that launched it, the one that the
    /// platform asks the user to trust.
    ///
    /// Call this first thing in `main`, before any thread starts: a platform that needs it
    /// may run the rest of the program in a new process.
    ///
    /// # Errors
    ///
    /// Fails when the platform can't make this program the one to trust, or can't tie the new
    /// process to the process that started it. The program can go on either way, and should log
    /// the error.
    fn take_responsibility() -> Result<(), ResponsibilityError> {
        Ok(())
    }

    /// Runs `work` on a new thread while the main thread serves the platform's event loop,
    /// and returns what `work` returns.
    ///
    /// Call this from `main`, and connect for [`Purpose::Capture`] inside `work`. A platform
    /// that delivers desktop events only to the main thread fails to connect for capture
    /// without it, with [`Error::MainThreadNotServed`].
    fn serve_main_thread<R: Send>(work: impl FnOnce() -> R + Send) -> R {
        work()
    }

    /// Reports how long a granted permission lasts for this program.
    #[must_use]
    fn access_lifetime() -> AccessLifetime {
        AccessLifetime::Durable
    }

    /// Lists the variables of the desktop session that decide whether applications expose
    /// their content. Empty on platforms where no variable decides it.
    #[must_use]
    fn session_variables() -> &'static [SessionVariable] {
        &[]
    }
}

/// A connection to the accessibility API of the desktop.
///
/// `Desktop` is the implementation for the current platform. The methods
/// that take `&self` can run concurrently, and every future is `Send`.
pub trait Backend: Platform + Sized + Send + Sync {
    /// A handle to a top-level window, or to a document that a window holds without showing
    /// it, such as a background tab. Two handles to the same window compare equal.
    type Window: Clone + Eq + Hash + Debug + Send + Sync;
    /// A handle to a running application.
    type App: Clone + Eq + Hash + Debug + Send + Sync;

    /// Connects to the accessibility API.
    ///
    /// Fails with [`Error::AccessDenied`] when [`access`](Platform::access) is denied.
    fn connect(purpose: Purpose) -> impl Future<Output = Result<Self, Error>> + Send;

    /// Asks running applications to expose their content.
    ///
    /// Some applications build their accessibility tree only once a client shows up. Returns
    /// how long those applications need, or `None` when every application was ready.
    fn wake_apps(&self) -> impl Future<Output = Option<Duration>> + Send;

    /// Lists the running applications that have an accessibility tree.
    fn apps(&self) -> impl Future<Output = Result<Vec<Self::App>, Error>> + Send;

    /// Lists the top-level windows of `app`.
    fn windows(
        &self,
        app: &Self::App,
    ) -> impl Future<Output = Result<Vec<Self::Window>, Error>> + Send;

    fn app_of(&self, window: &Self::Window) -> Self::App;

    /// The application that runs as the process `pid`.
    ///
    /// Capture calls this method for the process behind a [`Notice`]. A platform that cannot match
    /// processes to applications reports [`ProcessApp::NoApp`].
    fn app_of_process(&self, pid: u32) -> impl Future<Output = ProcessApp<Self::App>> + Send {
        let _ = pid;
        std::future::ready(ProcessApp::NoApp)
    }

    /// What the platform knows about the application of the app id `id`.
    ///
    /// Capture calls this method for the app id that a [`Notice`] declares, because users know
    /// applications by their names. A platform without app ids knows nothing.
    fn identify_app_id(&self, id: &str) -> impl Future<Output = AppIdentity<Self::App>> + Send {
        let _ = id;
        std::future::ready(AppIdentity::default())
    }

    /// Whether the application named `name` is a terminal emulator. Names are compared without
    /// regard to case.
    ///
    /// Capture doesn't follow terminals. A terminal shows shell history, which Atuin already
    /// captures, and the title of its window usually names the running command.
    fn is_terminal_name(name: &str) -> bool;

    /// Whether `app`, which [`app_name`](Self::app_name) named `name`, is a terminal emulator.
    ///
    /// A platform that knows more about an application than its name can tell more reliably
    /// than [`is_terminal_name`](Self::is_terminal_name).
    fn is_terminal(&self, app: &Self::App, name: &str) -> bool {
        let _ = app;
        Self::is_terminal_name(name)
    }

    /// Whether capture leaves out `app`, a part of the system whose windows hold nothing worth
    /// keeping.
    ///
    /// For example, macOS keeps its alert about the Accessibility permission open after the user
    /// grants the permission.
    fn is_left_out(&self, app: &Self::App) -> bool {
        let _ = app;
        false
    }

    fn app_name(&self, app: &Self::App) -> impl Future<Output = Result<String, Error>> + Send;

    fn active_window(&self) -> impl Future<Output = Option<Self::Window>> + Send;

    fn active_window_of(
        &self,
        app: &Self::App,
    ) -> impl Future<Output = Option<Self::Window>> + Send;

    /// Returns the top-level window that an activation of `window` refers to.
    ///
    /// Some toolkits announce activation on an inner object that holds none of the content.
    fn top_level(&self, window: Self::Window) -> impl Future<Output = Self::Window> + Send;

    /// Tells what kind of window `window` is to capture.
    fn window_class(
        &self,
        window: &Self::Window,
    ) -> impl Future<Output = Result<WindowClass, Error>> + Send;

    fn window_title(
        &self,
        window: &Self::Window,
    ) -> impl Future<Output = Result<String, Error>> + Send;

    /// Lists the pages that `window` holds without showing them, such as the background tabs
    /// of a browser that keeps them in its tree.
    ///
    /// [`read`](Self::read) reads each one whole, [`window_title`](Self::window_title) gives
    /// its title, and changes in it arrive as changes of its own window.
    fn hidden_documents(
        &self,
        window: &Self::Window,
    ) -> impl Future<Output = Vec<Self::Window>> + Send {
        let _ = window;
        async { Vec::new() }
    }

    /// Reads the content of `window` within `limits`.
    fn read(
        &self,
        window: &Self::Window,
        limits: Limits,
    ) -> impl Future<Output = Result<Reading, Error>> + Send;

    /// Waits for the next event that matters to capture. Cancelling the wait doesn't lose
    /// events.
    fn next(&mut self) -> impl Future<Output = Option<Event<Self>>> + Send;

    /// Stops the reading that the backend does on its own until [`resume`](Self::resume).
    ///
    /// A backend can follow the desktop between the calls of capture, for example by reading
    /// the notification banners that appear. A paused backend reads nothing from the
    /// applications. A backend that is connected for [`Purpose::Capture`] starts paused.
    fn pause(&self) {}

    /// Starts the reading that the backend does on its own, which capture needs while it runs.
    fn resume(&self) {}

    /// Takes the counts of every platform event received since the last call.
    fn take_event_counts(&mut self) -> impl Iterator<Item = EventCount<Self::App>> + use<Self>;
}

/// The diagnostics that tell why an application exposes little or nothing.
pub trait Inspect: Backend {
    /// Finds the running applications that don't expose their content until the user acts.
    ///
    /// Applications that [`wake_apps`](Backend::wake_apps) just asked count as unready until
    /// the wait that `wake_apps` returned is over.
    fn unready_apps(&self) -> impl Future<Output = Result<Vec<UnreadyApp>, Error>> + Send;

    /// Whether `app`, which [`app_name`](Backend::app_name) named `name`, shows nothing but the
    /// system's prompts, such as a prompt for a password.
    ///
    /// The text of such prompts says nothing about the user's work, so a suggestion of the
    /// applications to capture leaves these applications out.
    fn shows_only_prompts(&self, app: &Self::App, name: &str) -> bool {
        let _ = (app, name);
        false
    }

    fn app_info(&self, app: &Self::App) -> impl Future<Output = Result<AppInfo, Error>> + Send;

    /// Lists the unpruned element tree of `window` for diagnosis, without any of its text.
    ///
    /// The outline stops after `max_entries` elements and then reports itself truncated.
    fn outline(
        &self,
        window: &Self::Window,
        max_entries: usize,
    ) -> impl Future<Output = Result<Outline, Error>> + Send;
}
