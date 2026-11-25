//! Watches the desktop from the main thread: the frontmost application, the applications that
//! own windows, and the notification banners.

use std::ffi::c_void;
use std::ptr::{self, NonNull};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use atuin_common::collections::ForgetfulMap;
use atuin_common::os::macos::console::is_at_screen;
use block2::RcBlock;
use core_foundation::base::{CFType, TCFType};
use core_foundation::runloop::CFRunLoopTimerRef;
use core_foundation::string::{CFString, CFStringRef};
use objc2::rc::Retained;
use objc2_app_kit::{
    NSRunningApplication, NSWorkspace, NSWorkspaceApplicationKey,
    NSWorkspaceDidActivateApplicationNotification, NSWorkspaceDidLaunchApplicationNotification,
    NSWorkspaceDidTerminateApplicationNotification, NSWorkspaceSessionDidBecomeActiveNotification,
    NSWorkspaceSessionDidResignActiveNotification,
};
use objc2_foundation::{
    NSDistributedNotificationCenter, NSNotification, NSNotificationCenter, NSString,
};
use parking_lot::Mutex;
use tokio::sync::mpsc;

use super::desktop::{BundleId, BundleIds, focused_window, screen_now};
use super::dialogs::DialogProbe;
use super::element::{Element, Pid, str_of};
use super::observer::{
    AxNotification, AxObserver, Delivery, ObserveRetry, ScanTimer, Subscription, observe,
};
use super::owners::{Fronts, Observing, Owners};
use super::permission::{Prompt, is_trusted};
use super::wakes::WakeLog;
use super::{MacAx, Window, banners, ffi, main_thread, quirks};
use crate::backend::{Cause, Event, Screen};

/// How often the watcher looks for applications that came to own a window. No notification
/// announces a process that shows its first window without launching or activating.
const OWNER_SCAN: Duration = Duration::from_secs(1);
/// How soon the watcher scans again when the frontmost application isn't showing a window yet.
const EARLY_SCAN: Duration = Duration::from_millis(100);
/// How many early scans follow one another at most, before the scans keep [`OWNER_SCAN`].
const MAX_EARLY_SCANS: u32 = 10;
/// How many elements that announced a change of content the watcher remembers the windows of.
const REMEMBERED_WINDOWS: usize = 1_000;

/// The notifications that capture needs from the frontmost application.
const NOTIFICATIONS: [AxNotification; 3] = [
    AxNotification::FocusedWindowChanged,
    AxNotification::MainWindowChanged,
    AxNotification::TitleChanged,
];

/// State that the watcher on the main thread shares with the backend.
#[derive(Default)]
pub struct Shared {
    /// Whether capture is running. The watcher reads the desktop only when capture is running.
    running: AtomicBool,
    wakes: WakeLog,
    /// The focused window of the frontmost application, as of the watcher's last look.
    active: Mutex<Option<Window>>,
    bundles: BundleIds,
}

impl Shared {
    pub const fn bundles(&self) -> &BundleIds {
        &self.bundles
    }

    fn is_running(&self) -> bool {
        self.running.load(Ordering::Acquire)
    }

    /// Tells the watcher to stop reading the desktop, from its next callback on.
    pub fn pause(&self) {
        self.running.store(false, Ordering::Release);
    }

    /// Tells the watcher to read the desktop again, from its next callback on.
    pub fn resume(&self) {
        self.running.store(true, Ordering::Release);
    }

    /// Asks `app` to build its tree, and notes the first time that it was asked.
    pub fn wake(&self, app: Pid) {
        self.ask(app);
        self.wakes.note_first(app);
    }

    /// Forgets when the applications whose processes exited were asked to build their trees.
    pub fn forget_exited(&self) {
        self.wakes.forget_exited();
    }

    /// Asks `app` once more to build its tree, when [`quirks::WAKE_AGAIN`] has passed since
    /// the first request.
    pub fn wake_again_if_due(&self, app: Pid) {
        if self.wakes.again_is_due(app) {
            self.ask(app);
        }
    }

    fn ask(&self, app: Pid) {
        let bundle_id = self.bundles.get(app);
        quirks::wake(&Element::application(app), bundle_id.as_ref().map(BundleId::as_str));
    }

    /// Whether `app` may still be building its tree, because it was last asked to build it
    /// less than [`quirks::WAKE_TIME`] ago.
    pub fn may_be_building(&self, app: Pid) -> bool {
        self.wakes.may_be_building(app)
    }

    /// Returns the focused window of the frontmost application. Nothing is frontmost while the
    /// screen is locked.
    pub fn active_window(&self) -> Option<Window> {
        self.active.lock().clone()
    }

    /// Forgets what is known about `app`, whose process exited. A new process can reuse its id.
    fn forget(&self, app: Pid) {
        self.wakes.forget(app);
        self.bundles.forget(app);
    }
}

/// The application that the watcher currently observes.
struct Front {
    pid: Pid,
    _observer: AxObserver,
    window: Option<Element>,
}

impl Front {
    /// Follows the focused window of the application, and returns the event when it changed.
    fn window_changed(&mut self) -> Option<Event<MacAx>> {
        let window = focused_window(self.pid);
        if window == self.window {
            return None;
        }
        let previous = std::mem::replace(&mut self.window, window.clone());
        match (window, previous) {
            (Some(element), _) => Some(Event::Activated {
                window: Window::new(element, self.pid),
            }),
            (None, Some(element)) => Some(Event::Deactivated {
                window: Window::new(element, self.pid),
            }),
            (None, None) => None,
        }
    }

    /// The event for a new title of `element`, when `element` is the focused window.
    fn title_changed(&self, element: Element) -> Option<Event<MacAx>> {
        if self.window.as_ref() != Some(&element) {
            return None;
        }
        let title = element.string("AXTitle")?;
        Some(Event::TitleChanged {
            window: Window::new(element, self.pid),
            title,
        })
    }
}

/// The sender of the backend's events.
struct Events(Option<mpsc::UnboundedSender<Event<MacAx>>>);

impl Events {
    fn send(&self, event: Event<MacAx>) {
        // A send fails only when the backend is gone, and then the watcher is about to stop.
        if let Some(events) = &self.0 {
            let _ = events.send(event);
        }
    }

    /// Ends the backend's events.
    fn end(&mut self) {
        self.0 = None;
    }

    const fn have_ended(&self) -> bool {
        self.0.is_none()
    }
}

/// Paces the watcher's scans: once every [`OWNER_SCAN`], and sooner when the frontmost
/// application isn't showing a window yet.
struct ScanPacer {
    timer: Option<ScanTimer>,
    /// How many scans in a row came early.
    early_scans: u32,
}

impl ScanPacer {
    /// Brings the next scan forward, for a frontmost application that doesn't show a window
    /// yet, unless [`MAX_EARLY_SCANS`] came early in a row.
    ///
    /// UserNotificationCenter activates before its alert shows, and an alert can close within
    /// the second between two scans. Only a scan finds the alert, because the process doesn't
    /// have a focused window.
    fn scan_early(&mut self) {
        if self.early_scans >= MAX_EARLY_SCANS {
            return;
        }
        self.early_scans += 1;
        if let Some(timer) = &self.timer {
            timer.fire_in(EARLY_SCAN);
        }
    }

    const fn reset(&mut self) {
        self.early_scans = 0;
    }
}

pub struct Watcher {
    shared: Arc<Shared>,
    /// The watcher ends the events once this process loses the permission to use the AX API.
    events: Events,
    /// Where the observers of the watcher deliver their notifications. `start` points the
    /// context at this watcher.
    delivery: Delivery,
    front: Option<Front>,
    /// The state of the screen that the watcher last reported. `None` until the first look
    /// after a start or a pause.
    screen: Option<Screen>,
    owners: Owners,
    /// The applications that refused an observer, which the watcher backs off from.
    retry: ObserveRetry,
    subscriptions: Vec<Subscription>,
    scans: ScanPacer,
    banners: Option<Banners>,
    dialogs: DialogProbe,
    /// The window of each element that announced a change of content. An element announces
    /// many changes, and each lookup is an AX call on the main thread.
    ///
    /// An element can move into another window without a new window, such as a tab that the
    /// user drags into an existing window. The map is emptied on each look, which runs at
    /// least once every [`OWNER_SCAN`], and whenever an application's focused or main window
    /// changes, as it does when a tab is dropped into a window.
    windows_of: ForgetfulMap<Element, Element>,
}

/// The observer of Notification Center, which draws the notification banners.
struct Banners {
    app: Retained<NSRunningApplication>,
    pid: Pid,
    _observer: AxObserver,
    seen: banners::Seen,
}

/// A watcher on the main thread, which stops when the handle is dropped.
pub struct WatcherHandle(WatcherPtr);

impl WatcherHandle {
    /// Stops reading the desktop, and forgets what the watcher followed.
    ///
    /// `shared` must already tell that capture is paused, so that the watcher doesn't read
    /// anything before the pause reaches the main thread.
    pub fn pause(&self) {
        self.on_main_thread(Watcher::pause);
    }

    /// Looks at the desktop again. `shared` must already tell that capture is running.
    pub fn resume(&self) {
        self.on_main_thread(|watcher| watcher.look(None));
    }

    /// Runs `work` with the watcher on the main thread.
    fn on_main_thread(&self, work: impl FnOnce(&mut Watcher) + Send + 'static) {
        let watcher = self.0;
        main_thread::run(move || {
            // SAFETY: the pointer came from `Watcher::spawn`, and the main queue runs this after
            // `start`. The handle is live now, and dropping it queues `stop` after this.
            work(unsafe { &mut *watcher.get() });
        });
    }
}

impl Drop for WatcherHandle {
    fn drop(&mut self) {
        let watcher = self.0;
        main_thread::run(move || {
            // SAFETY: the pointer came from `Watcher::spawn`, the main queue runs this after
            // `start`, and nothing uses the pointer after the handle is dropped.
            unsafe { Watcher::stop(watcher.get()) };
        });
    }
}

/// The pointer to a watcher, which only the main thread dereferences.
#[derive(Clone, Copy)]
struct WatcherPtr(*mut Watcher);

// SAFETY: the pointer only travels to the main thread, where the watcher is used.
unsafe impl Send for WatcherPtr {}
// SAFETY: a shared pointer gives no access to the watcher.
unsafe impl Sync for WatcherPtr {}

impl WatcherPtr {
    /// Returns the pointer. Closures that call this capture the whole `WatcherPtr`, which is
    /// `Send`.
    const fn get(self) -> *mut Watcher {
        self.0
    }
}

/// What made the watcher look at the desktop again.
#[derive(Debug, Clone, Copy)]
enum LookCause {
    /// An application became active.
    Activated,
    /// An application quit.
    Terminated,
    /// An application launched, the screen locked or unlocked, or the session changed.
    Changed,
}

impl Watcher {
    /// Creates the watcher of a connection and starts it on the main thread.
    pub fn spawn(
        shared: Arc<Shared>,
        events: mpsc::UnboundedSender<Event<MacAx>>,
    ) -> WatcherHandle {
        let watcher = Box::new(Self {
            shared,
            events: Events(Some(events)),
            // SAFETY: no observer receives this delivery, because `start` replaces it before
            // the first call to `observe`.
            delivery: unsafe { Delivery::new(on_notification, ptr::null_mut()) },
            front: None,
            screen: None,
            owners: Owners::default(),
            retry: ObserveRetry::default(),
            subscriptions: Vec::new(),
            scans: ScanPacer {
                timer: None,
                early_scans: 0,
            },
            banners: None,
            dialogs: DialogProbe::default(),
            windows_of: ForgetfulMap::new(REMEMBERED_WINDOWS),
        });
        let watcher = WatcherPtr(Box::into_raw(watcher));
        main_thread::run(move || {
            // SAFETY: the pointer came from `Box::into_raw`, and only the main thread uses it
            // until `stop` frees it, which the main queue runs after this.
            unsafe { Self::start(watcher.get()) };
        });
        WatcherHandle(watcher)
    }

    /// Registers the observers of the watcher, and looks at the desktop for the first time.
    ///
    /// # Safety
    ///
    /// `raw` came from `Box::into_raw`, and this runs on the main thread.
    unsafe fn start(raw: *mut Self) {
        // SAFETY: as the caller guarantees.
        let watcher = unsafe { &mut *raw };
        // SAFETY: the main thread runs `on_notification`, and `stop` drops every observer, which
        // the watcher owns, before it frees the watcher.
        watcher.delivery = unsafe { Delivery::new(on_notification, raw.cast()) };
        let workspace = NSWorkspace::sharedWorkspace().notificationCenter();
        // SAFETY: the statics are valid constant strings for the lifetime of the process.
        let workspace_names = unsafe {
            [
                (NSWorkspaceDidActivateApplicationNotification, LookCause::Activated),
                (NSWorkspaceDidLaunchApplicationNotification, LookCause::Changed),
                (NSWorkspaceDidTerminateApplicationNotification, LookCause::Terminated),
                (NSWorkspaceSessionDidBecomeActiveNotification, LookCause::Changed),
                (NSWorkspaceSessionDidResignActiveNotification, LookCause::Changed),
            ]
        };
        for (name, cause) in workspace_names {
            watcher.subscribe(&workspace, name, cause);
        }
        let distributed = Retained::into_super(NSDistributedNotificationCenter::defaultCenter());
        for name in ["com.apple.screenIsLocked", "com.apple.screenIsUnlocked"] {
            watcher.subscribe(&distributed, &NSString::from_str(name), LookCause::Changed);
        }
        // SAFETY: the timer runs on the main thread, and the watcher owns the timer, which
        // `stop` invalidates before it frees the watcher.
        let timer = unsafe { ScanTimer::start(OWNER_SCAN, on_scan_timer, raw.cast()) };
        watcher.scans.timer = Some(timer);
        watcher.look(None);
    }

    /// Removes every observer of the watcher and frees it.
    ///
    /// # Safety
    ///
    /// `raw` came from [`spawn`](Self::spawn), and this runs on the main thread.
    unsafe fn stop(raw: *mut Self) {
        // SAFETY: as the caller guarantees. Dropping the watcher removes its observers, its
        // subscriptions, and its timer, and nothing else uses it.
        drop(unsafe { Box::from_raw(raw) });
    }

    /// Calls [`look`](Self::look) whenever `center` posts `name`.
    fn subscribe(
        &mut self,
        center: &Retained<NSNotificationCenter>,
        name: &NSString,
        cause: LookCause,
    ) {
        let raw = self.delivery.context().cast::<Self>();
        let block = RcBlock::new(move |notification: NonNull<NSNotification>| {
            // SAFETY: both centers post on the main thread, where nothing else borrows the
            // watcher, and `stop` removes the subscription before it frees the watcher.
            let watcher = unsafe { &mut *raw };
            // SAFETY: the notification is live for the duration of the call.
            let app = || notified_app(unsafe { notification.as_ref() });
            let activated = match cause {
                LookCause::Activated => app(),
                LookCause::Terminated => {
                    if let Some(app) = app() {
                        watcher.shared.forget(app);
                    }
                    None
                }
                LookCause::Changed => None,
            };
            watcher.look(activated);
        });
        // SAFETY: both centers post on the main thread, and the watcher owns the subscription.
        let subscription = unsafe { Subscription::new(center, name, &block) };
        self.subscriptions.push(subscription);
    }

    /// Follows the applications that own windows, and starts observing the frontmost
    /// application when it changed.
    ///
    /// `activated` is the application that just became active, when one did.
    fn look(&mut self, activated: Option<Pid>) {
        if self.events.have_ended() || !self.shared.is_running() {
            return;
        }
        // The AX API doesn't answer once the user revokes the permission, so capture would
        // only go quiet. Ending the events lets the helper report the missing permission.
        if !is_trusted(Prompt::Skip) {
            tracing::warn!("the Accessibility permission was revoked, so capture ends");
            self.end();
            return;
        }
        // The AX API reads only the session at the screen. A user who switched away, or who
        // logged out, ends capture as a Linux user without a desktop session does.
        if !is_at_screen() {
            tracing::debug!("this user left the screen, so capture ends");
            self.end();
            return;
        }
        self.windows_of.clear();
        // Nobody works in a locked session, so capture stays idle. The windows of a session
        // remain readable while the session is off the console, and windows in every session
        // read as empty while the session on the console is locked.
        let screen = screen_now();
        self.report_screen(screen);
        self.owners.scan(screen, &self.shared.bundles);
        // Nothing is frontmost while the screen is locked.
        let pid = match screen {
            Screen::Locked => None,
            Screen::Unlocked => activated.or_else(|| {
                let app = NSWorkspace::sharedWorkspace().frontmostApplication()?;
                Some(Pid::new(app.processIdentifier()))
            }),
        };
        let fronts = Fronts {
            now: pid,
            before: self.front.as_ref().map(|front| front.pid),
        };
        let observing = Observing {
            delivery: self.delivery,
            retry: &mut self.retry,
            bundles: &self.shared.bundles,
        };
        for &app in self.owners.follow(screen, fronts, observing) {
            self.events.send(Event::Joined { app });
        }
        self.follow_banners();
        self.follow_front(pid);
        self.publish_active();
        if self.front.as_ref().is_some_and(|front| front.window.is_none()) {
            self.scans.scan_early();
        } else {
            self.scans.reset();
        }
    }

    /// Reports `screen` when it differs from the state that the watcher last reported.
    ///
    /// The report goes out before the events of the look, so that capture stops before the
    /// events of a locked screen, and reads the desktop again only after the unlock.
    fn report_screen(&mut self, screen: Screen) {
        if self.screen.replace(screen) != Some(screen) {
            tracing::debug!(?screen, "the state of the screen changed");
            self.events.send(Event::ScreenChanged { screen });
        }
    }

    /// Stops following the desktop, and ends the backend's events.
    ///
    /// The subscriptions and the scan timer stay, because this runs inside one of their
    /// callbacks. Each later look returns at once.
    fn end(&mut self) {
        self.events.end();
        self.forget_followed();
        self.owners.stop_observing();
    }

    /// Stops observing the applications, and forgets what the watcher followed. The next look
    /// starts over, as the first one does.
    fn pause(&mut self) {
        self.screen = None;
        self.forget_followed();
        self.owners.start_over();
        self.scans.reset();
    }

    /// Drops the observers of the frontmost application and of the banners, and publishes that
    /// no window is active.
    fn forget_followed(&mut self) {
        self.front = None;
        self.banners = None;
        self.windows_of.clear();
        self.publish_active();
    }

    /// Stores the focused window of the frontmost application for
    /// [`Shared::active_window`].
    fn publish_active(&self) {
        let active = self.front.as_ref().and_then(|front| {
            let element = front.window.clone()?;
            Some(Window::new(element, front.pid))
        });
        *self.shared.active.lock() = active;
    }

    fn follow_front(&mut self, pid: Option<Pid>) {
        let now = Instant::now();
        if let Some(front) = &mut self.front
            && Some(front.pid) == pid
        {
            // An application that refused an observer can hang, and each AX call then blocks
            // for its timeout.
            if self.retry.refused(front.pid) {
                return;
            }
            self.shared.wake_again_if_due(front.pid);
            // An application that activates while it launches doesn't have a window yet, and
            // may not announce the window when it comes. An application whose last window
            // closes may not announce that either.
            if let Some(event) = front.window_changed() {
                self.events.send(event);
            }
            return;
        }
        // Taking the old front drops its observer.
        if let Some(old) = self.front.take()
            && let Some(window) = old.window
        {
            self.events.send(Event::Deactivated {
                window: Window::new(window, old.pid),
            });
        }
        let Some(pid) = pid else {
            return;
        };
        if self.retry.backs_off(pid, now) {
            return;
        }
        self.shared.forget_exited();
        self.shared.wake(pid);
        let delivery = self.delivery;
        let Some(observer) =
            self.retry.attempt(pid, now, || observe(pid, &NOTIFICATIONS, delivery))
        else {
            return;
        };
        let observing = Observing {
            delivery,
            retry: &mut self.retry,
            bundles: &self.shared.bundles,
        };
        self.owners.observe_front(pid, observing, now);
        let window = focused_window(pid);
        if let Some(window) = &window {
            self.events.send(Event::Activated {
                window: Window::new(window.clone(), pid),
            });
        }
        self.front = Some(Front {
            pid,
            _observer: observer,
            window,
        });
        self.scans.reset();
    }

    /// Observes Notification Center, and follows it when it restarts.
    fn follow_banners(&mut self) {
        if self.banners.as_ref().is_some_and(|banners| !banners.app.isTerminated()) {
            return;
        }
        self.banners = None;
        let id = NSString::from_str(banners::BUNDLE_ID);
        let Some(app) =
            NSRunningApplication::runningApplicationsWithBundleIdentifier(&id).firstObject()
        else {
            return;
        };
        let pid = Pid::new(app.processIdentifier());
        if let Some(observer) = observe(pid, &banners::NOTIFICATIONS, self.delivery) {
            self.banners = Some(Banners {
                app,
                pid,
                _observer: observer,
                seen: banners::Seen::default(),
            });
        }
    }

    fn notified(&mut self, element: Element, notification: &str) {
        // A notification that was queued before a pause can arrive after it.
        if !self.shared.is_running() {
            return;
        }
        if self.banners.as_ref().is_some_and(|banners| element.pid() == Some(banners.pid)) {
            self.on_banner(&element);
            return;
        }
        let Some(notification) = AxNotification::parse(notification) else {
            return;
        };
        match notification {
            AxNotification::FocusedUiElementChanged => self.on_focus_moved(element),
            AxNotification::WindowCreated | AxNotification::SheetCreated => {
                let Some(pid) = element.pid() else {
                    return;
                };
                // A new window can take elements from another one. For example, a tab that the
                // user drags out of its window moves into a new window.
                self.windows_of.clear();
                self.events.send(Event::Opened {
                    window: Window::new(element, pid),
                });
            }
            AxNotification::ValueChanged
            | AxNotification::LoadComplete
            | AxNotification::LayoutChanged => {
                let Some(pid) = element.pid() else {
                    return;
                };
                let cause = match notification {
                    AxNotification::ValueChanged => Cause::Edited,
                    AxNotification::LoadComplete => Cause::Loaded,
                    _ => Cause::Restructured,
                };
                let window = self.window_of(&element);
                self.events.send(Event::ContentChanged {
                    app: pid,
                    window: window.map(|element| Window::new(element, pid)),
                    cause,
                });
            }
            AxNotification::FocusedWindowChanged | AxNotification::MainWindowChanged => {
                self.windows_of.clear();
                self.on_front(element, notification);
            }
            AxNotification::TitleChanged => self.on_front(element, notification),
        }
    }

    /// Returns the window that holds `element`, or `element` itself when it is a window.
    fn window_of(&mut self, element: &Element) -> Option<Element> {
        if let Some(window) = self.windows_of.get(element) {
            return Some(window.clone());
        }
        // A window doesn't report an `AXWindow` of its own.
        let window = element
            .element("AXWindow")
            .or_else(|| element.has("AXRole", "AXWindow").then(|| element.clone()))?;
        self.windows_of.insert(element.clone(), window.clone());
        Some(window)
    }

    /// Reads a banner that Notification Center changed.
    fn on_banner(&mut self, element: &Element) {
        let Some(banners) = &mut self.banners else {
            return;
        };
        if let Some(notice) = banners.seen.read(element) {
            self.events.send(Event::Notified { notice });
        }
    }

    /// Announces the dialog group inside a window that took the focus.
    ///
    /// Safari and Firefox show a page's alert as a dialog group inside the window, and announce
    /// it only by moving the focus into it.
    fn on_focus_moved(&self, element: Element) {
        let Some(pid) = element.pid() else {
            return;
        };
        if let Some(dialog) = self.dialogs.enclosing_dialog(element) {
            self.events.send(Event::Opened {
                window: Window::new(dialog, pid),
            });
        }
    }

    /// Follows a change of the focused window or of the title of the frontmost application.
    fn on_front(&mut self, element: Element, notification: AxNotification) {
        let Some(front) = &mut self.front else {
            return;
        };
        let event = if notification == AxNotification::TitleChanged {
            front.title_changed(element)
        } else {
            front.window_changed()
        };
        if let Some(event) = event {
            self.publish_active();
            self.events.send(event);
        }
    }
}

extern "C" fn on_scan_timer(_timer: CFRunLoopTimerRef, info: *mut c_void) {
    // SAFETY: `info` is the watcher pointer that `start` registered. The run loop calls this
    // function on the main thread, where nothing else borrows the watcher, and the watcher
    // invalidates the timer before it is freed.
    let watcher = unsafe { &mut *info.cast::<Watcher>() };
    watcher.look(None);
}

/// Returns the application that a workspace notification names, such as the application that
/// became active.
fn notified_app(notification: &NSNotification) -> Option<Pid> {
    let info = notification.userInfo()?;
    // SAFETY: the static is a valid constant string for the lifetime of the process.
    let key = unsafe { NSWorkspaceApplicationKey };
    let app = info.objectForKey(key)?.downcast::<NSRunningApplication>().ok()?;
    Some(Pid::new(app.processIdentifier()))
}

extern "C" fn on_notification(
    _observer: ffi::AXObserverRef,
    element: ffi::AXUIElementRef,
    notification: CFStringRef,
    context: *mut c_void,
) {
    // SAFETY: `context` is the watcher pointer of the watcher's delivery. The run loop calls
    // this function on the main thread, where nothing else borrows the watcher, and the watcher
    // removes its observers before it is freed.
    let watcher = unsafe { &mut *context.cast::<Watcher>() };
    // SAFETY: the element is live for the duration of the callback, and wrapping retains it.
    let Some(element) = Element::from_value(unsafe { CFType::wrap_under_get_rule(element) }) else {
        return;
    };
    // SAFETY: the notification name is live for the duration of the callback, and wrapping
    // retains it.
    let notification = unsafe { CFString::wrap_under_get_rule(notification) };
    watcher.notified(element, &str_of(&notification));
}
