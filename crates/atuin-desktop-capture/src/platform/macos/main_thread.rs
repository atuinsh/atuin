//! The main thread's run loop, which is the only one that receives `NSWorkspace`
//! notifications.
//!
//! On a process without `NSApplication`, `NSWorkspace` posts its notifications only while the
//! main thread is running its run loop, regardless of which thread or queue registered the
//! observer.
//! `frontmostApplication` also stays current only then.

use std::sync::atomic::{AtomicBool, Ordering};
use std::{panic, ptr};

use core_foundation::base::TCFType;
use core_foundation::date::CFAbsoluteTimeGetCurrent;
use core_foundation::runloop::{
    CFRunLoop, CFRunLoopAddTimer, CFRunLoopGetMain, CFRunLoopRunInMode, CFRunLoopTimer,
    CFRunLoopTimerRef, kCFRunLoopDefaultMode,
};
use dispatch2::DispatchQueue;

/// Whether the main thread is serving its run loop.
static SERVED: AtomicBool = AtomicBool::new(false);

/// The longest single wait of the run loop, in seconds. The loop waits again after it.
const WAIT_SECS: f64 = 1e9;

/// Runs `work` on a new thread while the main thread runs its run loop.
///
/// # Panics
///
/// Panics when called on a thread other than the main thread, and when `work` panics.
pub fn serve<R: Send>(work: impl FnOnce() -> R + Send) -> R {
    // SAFETY: the function has no preconditions.
    let on_main_thread = unsafe { libc::pthread_main_np() } == 1;
    assert!(on_main_thread, "the main thread's run loop must run on the main thread");
    // A run loop without sources returns at once, so a timer that never fires keeps it
    // waiting.
    let keepalive = CFRunLoopTimer::new(
        // SAFETY: the function has no preconditions.
        unsafe { CFAbsoluteTimeGetCurrent() } + WAIT_SECS,
        WAIT_SECS,
        0,
        0,
        never_fires,
        ptr::null_mut(),
    );
    // SAFETY: the function has no preconditions.
    let main = unsafe { CFRunLoopGetMain() };
    // SAFETY: the timer is live, the main run loop exists for the life of the process, and
    // the mode is a valid constant.
    unsafe { CFRunLoopAddTimer(main, keepalive.as_concrete_TypeRef(), kCFRunLoopDefaultMode) };
    SERVED.store(true, Ordering::Release);
    let done = AtomicBool::new(false);
    let result = std::thread::scope(|scope| {
        let worker = scope.spawn(|| {
            let _finished = Finished(&done);
            work()
        });
        while !done.load(Ordering::Acquire) {
            // SAFETY: the mode is a valid constant. The observers that the watcher registered
            // run inside this call.
            unsafe { CFRunLoopRunInMode(kCFRunLoopDefaultMode, WAIT_SECS, 0) };
        }
        worker.join()
    });
    SERVED.store(false, Ordering::Release);
    result.unwrap_or_else(|payload| panic::resume_unwind(payload))
}

/// Reports whether [`serve`] is running.
pub fn is_served() -> bool {
    SERVED.load(Ordering::Acquire)
}

/// Runs `work` on the main thread, after the work that is already queued there.
pub fn run(work: impl FnOnce() + Send + 'static) {
    DispatchQueue::main().exec_async(work);
}

/// Ends the run loop of [`serve`] when the work is done, including when the work panics.
struct Finished<'a>(&'a AtomicBool);

impl Drop for Finished<'_> {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
        // A stop from this thread is lost when it comes after the main thread checked `done`
        // and before the run loop started its next run. The main queue runs the stop inside
        // the next run instead, whenever that starts.
        run(|| CFRunLoop::get_current().stop());
    }
}

extern "C" fn never_fires(_timer: CFRunLoopTimerRef, _info: *mut std::ffi::c_void) {}
