//! What the watcher keeps on the run loop of the main thread: observers of the AX API,
//! subscriptions to notification centers, and the timer of its scans.

use std::collections::HashMap;
use std::ffi::c_void;
use std::num::NonZeroU32;
use std::ptr::{self, NonNull};
use std::time::{Duration, Instant};

use atuin_common::futures::{Backoff, Delays};
use block2::RcBlock;
use core_foundation::base::{CFType, TCFType};
use core_foundation::date::CFAbsoluteTimeGetCurrent;
use core_foundation::runloop::{
    CFRunLoop, CFRunLoopAddSource, CFRunLoopGetCurrent, CFRunLoopRemoveSource, CFRunLoopTimer,
    CFRunLoopTimerCallBack, CFRunLoopTimerContext, CFRunLoopTimerInvalidate,
    CFRunLoopTimerSetNextFireDate, kCFRunLoopDefaultMode,
};
use core_foundation::string::CFString;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_foundation::{NSNotification, NSNotificationCenter, NSObjectProtocol, NSString};

use super::element::{Element, Pid};
use super::ffi;

/// The notifications of the AX API that the watcher follows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AxNotification {
    FocusedWindowChanged,
    MainWindowChanged,
    TitleChanged,
    WindowCreated,
    SheetCreated,
    FocusedUiElementChanged,
    ValueChanged,
    LoadComplete,
    LayoutChanged,
}

impl AxNotification {
    const ALL: [Self; 9] = [
        Self::FocusedWindowChanged,
        Self::MainWindowChanged,
        Self::TitleChanged,
        Self::WindowCreated,
        Self::SheetCreated,
        Self::FocusedUiElementChanged,
        Self::ValueChanged,
        Self::LoadComplete,
        Self::LayoutChanged,
    ];

    const fn name(self) -> &'static str {
        match self {
            Self::FocusedWindowChanged => "AXFocusedWindowChanged",
            Self::MainWindowChanged => "AXMainWindowChanged",
            Self::TitleChanged => "AXTitleChanged",
            Self::WindowCreated => "AXWindowCreated",
            Self::SheetCreated => "AXSheetCreated",
            Self::FocusedUiElementChanged => "AXFocusedUIElementChanged",
            Self::ValueChanged => "AXValueChanged",
            Self::LoadComplete => "AXLoadComplete",
            Self::LayoutChanged => "AXLayoutChanged",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|notification| notification.name() == name)
    }
}

/// Where the observers deliver their notifications: `callback` receives `context` with each
/// one.
#[derive(Clone, Copy)]
pub struct Delivery {
    callback: ffi::AXObserverCallback,
    context: *mut c_void,
}

impl Delivery {
    /// A delivery of notifications to `callback` with `context`.
    ///
    /// # Safety
    ///
    /// `callback` may be called with `context` on this thread for as long as any observer that
    /// [`observe`] creates for this delivery lives.
    pub const unsafe fn new(callback: ffi::AXObserverCallback, context: *mut c_void) -> Self {
        Self { callback, context }
    }

    pub const fn context(&self) -> *mut c_void {
        self.context
    }
}

/// An `AXObserver` on the run loop of this thread, which leaves the run loop when dropped.
pub struct AxObserver(CFType);

impl Drop for AxObserver {
    fn drop(&mut self) {
        // SAFETY: the observer is live.
        let source = unsafe { ffi::AXObserverGetRunLoopSource(self.0.as_CFTypeRef()) };
        // SAFETY: every thread has a run loop.
        let run_loop = unsafe { CFRunLoopGetCurrent() };
        // SAFETY: the run loop and the source are live, and the mode is a valid constant.
        unsafe { CFRunLoopRemoveSource(run_loop, source, kCFRunLoopDefaultMode) };
    }
}

/// Creates an observer of `notifications` on the application `pid`, and adds it to the run loop
/// of this thread.
///
/// Returns `None` when the application as a whole can't answer, as happens for a while after
/// it launches, when it hangs, and when this process has lost the permission to use the AX API.
/// A notification that the application refuses for another reason is skipped.
pub fn observe(
    pid: Pid,
    notifications: &[AxNotification],
    delivery: Delivery,
) -> Option<AxObserver> {
    let app = Element::application(pid);
    let mut observer = ptr::null();
    // SAFETY: `observer` outlives the call, and the callback matches the expected signature.
    let error = unsafe { ffi::AXObserverCreate(pid.raw(), delivery.callback, &raw mut observer) };
    if error != ffi::AX_SUCCESS || observer.is_null() {
        return None;
    }
    // SAFETY: a successful call stores an observer that the caller owns.
    let observer = unsafe { CFType::wrap_under_create_rule(observer) };
    for notification in notifications {
        let name = CFString::from_static_string(notification.name());
        // SAFETY: the observer, the element, and the name are live. The creator of `delivery`
        // keeps the context valid for as long as the observer lives.
        let error = unsafe {
            ffi::AXObserverAddNotification(
                observer.as_CFTypeRef(),
                app.as_ptr(),
                name.as_concrete_TypeRef(),
                delivery.context,
            )
        };
        match error {
            ffi::AX_SUCCESS | ffi::AX_NOTIFICATION_UNSUPPORTED => {}
            error if app_cannot_answer(error) => {
                tracing::debug!(pid = pid.raw(), error, "the application refused an observer");
                return None;
            }
            error => {
                let notification = notification.name();
                tracing::debug!(pid = pid.raw(), error, notification, "skipping a notification");
            }
        }
    }
    // SAFETY: the observer is live.
    let source = unsafe { ffi::AXObserverGetRunLoopSource(observer.as_CFTypeRef()) };
    // SAFETY: every thread has a run loop.
    let run_loop = unsafe { CFRunLoopGetCurrent() };
    // SAFETY: the run loop and the source are live, and the mode is a valid constant.
    unsafe { CFRunLoopAddSource(run_loop, source, kCFRunLoopDefaultMode) };
    Some(AxObserver(observer))
}

/// Whether `error` means that the application as a whole can't answer, so that every observer
/// of the application fails.
const fn app_cannot_answer(error: ffi::AXError) -> bool {
    matches!(error, ffi::AX_CANNOT_COMPLETE | ffi::AX_INVALID_UI_ELEMENT | ffi::AX_API_DISABLED)
}

/// How [`ObserveRetry`] spaces its attempts to observe an application that refused an
/// observer. The first retry comes at the next scan of the watcher.
///
/// Each attempt can block the main thread for the length of an AX call's timeout, if the
/// application hangs.
const OBSERVE_RETRY: Backoff = Backoff::Exponential {
    initial: Duration::from_secs(1),
    max: Duration::from_secs(30),
    factor: NonZeroU32::new(2).unwrap(),
};

/// The applications that refused an observer, with when to try each one again.
#[derive(Default)]
pub struct ObserveRetry(HashMap<Pid, Refused>);

/// An application that refused an observer.
struct Refused {
    retry_at: Instant,
    delays: Delays,
}

impl ObserveRetry {
    /// Whether the watcher waits before it asks `app` again, because `app` refused an observer.
    pub fn backs_off(&self, app: Pid, now: Instant) -> bool {
        self.0.get(&app).is_some_and(|refused| refused.retry_at > now)
    }

    /// Whether `app` refused its last observer. Such an application can hang, and each AX call
    /// to it then blocks for its timeout.
    pub fn refused(&self, app: Pid) -> bool {
        self.0.contains_key(&app)
    }

    /// Runs `attempt` to observe `app`, and spaces the next attempts by [`OBSERVE_RETRY`] for as
    /// long as `app` keeps refusing.
    pub fn attempt(
        &mut self,
        app: Pid,
        now: Instant,
        attempt: impl FnOnce() -> Option<AxObserver>,
    ) -> Option<AxObserver> {
        let observer = attempt();
        if observer.is_some() {
            self.0.remove(&app);
        } else {
            let refused = self.0.entry(app).or_insert_with(|| Refused {
                retry_at: now,
                delays: OBSERVE_RETRY.delays(),
            });
            refused.retry_at = now + refused.delays.next_delay();
        }
        observer
    }

    /// Forgets the applications that `kept` rejects.
    pub fn retain(&mut self, kept: impl Fn(&Pid) -> bool) {
        self.0.retain(|pid, _| kept(pid));
    }
}

/// An observer that a notification center calls, removed when dropped.
pub struct Subscription {
    center: Retained<NSNotificationCenter>,
    token: Retained<ProtocolObject<dyn NSObjectProtocol>>,
}

impl Subscription {
    /// Calls `block` on the posting thread whenever `center` posts `name`.
    ///
    /// # Safety
    ///
    /// `block` may run on every thread where `center` posts `name`, for as long as the
    /// subscription lives.
    pub unsafe fn new(
        center: &Retained<NSNotificationCenter>,
        name: &NSString,
        block: &RcBlock<dyn Fn(NonNull<NSNotification>)>,
    ) -> Self {
        // SAFETY: the block is called on the posting thread, where the caller allows it to run.
        let token = unsafe {
            center.addObserverForName_object_queue_usingBlock(Some(name), None, None, block)
        };
        Self {
            center: center.clone(),
            token,
        }
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        // SAFETY: the token came from this center.
        unsafe { self.center.removeObserver(self.token.as_ref()) };
    }
}

/// A timer on the run loop of this thread, which stops when dropped.
pub struct ScanTimer(CFRunLoopTimer);

impl ScanTimer {
    /// Starts a timer that calls `callback` with `info` every `interval`.
    ///
    /// # Safety
    ///
    /// `callback` may be called with `info` on this thread for as long as the timer lives.
    pub unsafe fn start(
        interval: Duration,
        callback: CFRunLoopTimerCallBack,
        info: *mut c_void,
    ) -> Self {
        let mut context = CFRunLoopTimerContext {
            version: 0,
            info,
            retain: None,
            release: None,
            copyDescription: None,
        };
        let interval = interval.as_secs_f64();
        // The timer copies the context.
        let timer = CFRunLoopTimer::new(
            // SAFETY: the function has no preconditions.
            unsafe { CFAbsoluteTimeGetCurrent() } + interval,
            interval,
            0,
            0,
            callback,
            &raw mut context,
        );
        // SAFETY: the mode is a valid constant.
        CFRunLoop::get_current().add_timer(&timer, unsafe { kCFRunLoopDefaultMode });
        Self(timer)
    }

    /// Brings the next call forward, to `delay` from now.
    pub fn fire_in(&self, delay: Duration) {
        // SAFETY: the function has no preconditions.
        let now = unsafe { CFAbsoluteTimeGetCurrent() };
        // SAFETY: the timer is live.
        unsafe {
            CFRunLoopTimerSetNextFireDate(self.0.as_concrete_TypeRef(), now + delay.as_secs_f64());
        }
    }
}

impl Drop for ScanTimer {
    fn drop(&mut self) {
        // SAFETY: the timer is live.
        unsafe { CFRunLoopTimerInvalidate(self.0.as_concrete_TypeRef()) };
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case::cannot_complete(ffi::AX_CANNOT_COMPLETE, true)]
    #[case::invalid_element(ffi::AX_INVALID_UI_ELEMENT, true)]
    #[case::api_disabled(ffi::AX_API_DISABLED, true)]
    #[case::unsupported(ffi::AX_NOTIFICATION_UNSUPPORTED, false)]
    #[case::failure(-25200, false)]
    #[case::already_registered(-25209, false)]
    fn only_errors_of_the_whole_application_drop_it(
        #[case] error: ffi::AXError,
        #[case] expected: bool,
    ) {
        assert_eq!(app_cannot_answer(error), expected);
    }

    /// An application that refuses waits about a second, then about two, before the next
    /// attempt. Each wait is jittered by up to a tenth.
    #[rstest]
    fn a_refusal_spaces_the_next_attempts() {
        let app = Pid::new(7);
        let start = Instant::now();
        let mut retry = ObserveRetry::default();

        assert!(retry.attempt(app, start, || None).is_none());
        assert!(retry.refused(app));
        assert!(retry.backs_off(app, start + Duration::from_millis(850)));
        assert!(!retry.backs_off(app, start + Duration::from_millis(1150)));

        let again = start + Duration::from_millis(1150);
        assert!(retry.attempt(app, again, || None).is_none());
        assert!(retry.backs_off(app, again + Duration::from_millis(1700)));
        assert!(!retry.backs_off(app, again + Duration::from_millis(2300)));
    }

    #[rstest]
    fn a_forgotten_application_is_tried_at_once() {
        let app = Pid::new(7);
        let now = Instant::now();
        let mut retry = ObserveRetry::default();
        let _ = retry.attempt(app, now, || None);

        retry.retain(|&pid| pid != app);

        assert!(!retry.refused(app));
        assert!(!retry.backs_off(app, now));
    }
}
