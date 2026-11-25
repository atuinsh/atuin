//! Watches cheaply for desktop sessions that start while capture is waiting for one, so that the
//! supervisor starts a helper when a session may have started rather than at fixed intervals.
//!
//! - `display_sockets`: on the Unix systems other than macOS, the compositor or the X server of
//!   a session creates a display socket. The watch can only guess whether a helper finds the
//!   session.
//! - `console`: on macOS, the user at the screen owns the console, and a helper finds a desktop
//!   exactly when the user is at the screen.

#[cfg(any(target_os = "macos", test))]
mod console;
#[cfg(all(unix, not(target_os = "macos")))]
mod display_sockets;

use std::time::Duration;

use futures::future::BoxFuture;

/// How often a watch looks at the system.
///
/// Each look takes a few system calls, so a watch costs little however long it runs.
#[cfg_attr(all(not(unix), not(test)), expect(dead_code, reason = "only the Unix watches look"))]
const POLL: Duration = Duration::from_secs(2);

/// Whether a helper would find a desktop session now, as far as a watch can tell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(
    all(not(target_os = "macos"), not(test)),
    expect(dead_code, reason = "only the watch on macOS can tell")
)]
pub enum Desktop {
    Present,
    Absent,
    /// The watch can't tell. Only a helper can find out.
    #[cfg_attr(
        all(target_os = "macos", not(test)),
        expect(dead_code, reason = "the watch on macOS can always tell")
    )]
    Unknown,
}

/// Tells when a desktop session may have started.
pub trait SessionWatch: Send + Sync {
    /// Whether a helper would find a desktop session now.
    ///
    /// A watch that can tell answers [`Desktop::Present`] or [`Desktop::Absent`]. The
    /// [`started`](Self::started) of such a watch resolves once the session is present.
    fn desktop(&self) -> Desktop;

    /// Starts a new watch. [`started`](Self::started) counts only the sessions that start after
    /// the mark. A watch whose [`desktop`](Self::desktop) isn't [`Desktop::Unknown`] ignores
    /// marks.
    fn mark(&mut self);

    /// Resolves once a desktop session may have started since the last [`mark`](Self::mark).
    /// When [`desktop`](Self::desktop) isn't [`Desktop::Unknown`], the future resolves whenever a
    /// desktop session is present, at once when one is present already.
    fn started(&mut self) -> BoxFuture<'_, ()>;
}

/// The watch of this platform.
#[must_use]
pub fn for_platform() -> Box<dyn SessionWatch> {
    #[cfg(target_os = "macos")]
    let watch = console::Console::new(atuin_common::os::macos::console::is_at_screen);
    #[cfg(all(unix, not(target_os = "macos")))]
    let watch = display_sockets::DisplaySockets::of_this_user();
    #[cfg(not(unix))]
    let watch = Unwatched;
    Box::new(watch)
}

/// A watch that never sees a session start, for the platforms without a helper. Only the sessions
/// that the autostart entry reports through `atuin desktop-capture login` then end a wait for a
/// desktop session.
#[cfg(not(unix))]
struct Unwatched;

#[cfg(not(unix))]
impl SessionWatch for Unwatched {
    fn desktop(&self) -> Desktop {
        Desktop::Unknown
    }

    fn mark(&mut self) {}

    fn started(&mut self) -> BoxFuture<'_, ()> {
        Box::pin(futures::future::pending())
    }
}
