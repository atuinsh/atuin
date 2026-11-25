//! The watch on the Unix systems with Wayland or X11, where the compositor or the X server of
//! each session creates a display socket.

use std::time::SystemTime;

use atuin_common::os::unix::display::DisplayDirs;
use futures::future::BoxFuture;
use tracing::debug;

use super::{Desktop, POLL, SessionWatch};

/// Sees the display sockets that the compositors and X servers of this user's sessions create.
///
/// The daemon's environment usually lacks `WAYLAND_DISPLAY` and `DISPLAY`, so the watch looks
/// for every display socket that may belong to a session of this user. That includes the
/// sockets of X servers that run as root, such as a display manager's.
#[derive(Debug)]
pub struct DisplaySockets {
    dirs: DisplayDirs,
    uid: u32,
    /// The time of the last mark. A socket counts when it was created after this time.
    since: SystemTime,
}

impl DisplaySockets {
    pub fn of_this_user() -> Self {
        Self::in_dirs(DisplayDirs::of_this_process(), atuin_common::os::unix::uid())
    }

    const fn in_dirs(dirs: DisplayDirs, uid: u32) -> Self {
        Self {
            dirs,
            uid,
            since: SystemTime::UNIX_EPOCH,
        }
    }

    /// Whether a display socket that may belong to a session of this user appeared since the
    /// last mark.
    fn has_new_socket(&self) -> bool {
        self.dirs
            .sockets_for(self.uid)
            .any(|socket| socket.modified().is_ok_and(|modified| modified > self.since))
    }
}

impl SessionWatch for DisplaySockets {
    /// A display socket doesn't tell whether its session lets a helper in, so only a helper can
    /// find out.
    fn desktop(&self) -> Desktop {
        Desktop::Unknown
    }

    fn mark(&mut self) {
        self.since = SystemTime::now();
    }

    fn started(&mut self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            while !self.has_new_socket() {
                tokio::time::sleep(POLL).await;
            }
            debug!("a display socket appeared, so a desktop session may have started");
        })
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::net::UnixListener;
    use std::time::Duration;

    use rstest::{fixture, rstest};
    use tempfile::TempDir;

    use super::*;

    /// A watch of an empty runtime directory, and the directory until it drops.
    struct Watched {
        runtime: TempDir,
        watch: DisplaySockets,
    }

    #[fixture]
    fn watched() -> Watched {
        let runtime = tempfile::tempdir().expect("tempdir");
        let dirs = DisplayDirs {
            runtime: runtime.path().to_path_buf(),
            x11: runtime.path().join("x11"),
        };
        Watched {
            watch: DisplaySockets::in_dirs(dirs, atuin_common::os::unix::uid()),
            runtime,
        }
    }

    impl Watched {
        fn socket(&self, name: &str) {
            UnixListener::bind(self.runtime.path().join(name)).expect("bind a socket");
        }
    }

    /// Waits until the file times of new files are later than the time of a mark that came
    /// before them. The kernel stamps files with a coarse clock, which can lag the clock that
    /// the mark reads by a few milliseconds.
    fn after_the_mark() {
        std::thread::sleep(Duration::from_millis(50));
    }

    #[rstest]
    fn a_socket_from_after_the_mark_counts(mut watched: Watched) {
        watched.watch.mark();
        after_the_mark();

        watched.socket("wayland-1");

        assert!(watched.watch.has_new_socket());
    }

    #[rstest]
    fn a_socket_from_before_the_mark_does_not_count(mut watched: Watched) {
        watched.socket("wayland-0");
        after_the_mark();

        watched.watch.mark();

        assert!(!watched.watch.has_new_socket());
    }

    #[rstest]
    #[tokio::test]
    async fn started_resolves_once_a_socket_appears(mut watched: Watched) {
        watched.watch.mark();
        after_the_mark();
        watched.socket("wayland-2");

        tokio::time::timeout(Duration::from_secs(10), watched.watch.started())
            .await
            .expect("the watch saw the socket");
    }
}
