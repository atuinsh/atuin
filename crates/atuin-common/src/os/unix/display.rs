//! The display sockets that Wayland compositors and X servers create.
//!
//! A compositor names its socket `wayland-<n>` in the user's runtime directory, as gamescope
//! names its `gamescope-<n>`, and an X server names its socket `X<n>` in [`X11_SOCKET_DIR`],
//! which the X servers of every user share.

use std::fs::{DirEntry, Metadata};
use std::os::unix::fs::{FileTypeExt as _, MetadataExt as _};
use std::path::{Path, PathBuf};

use crate::path::AbsPathBuf;

/// The directory of the X servers' sockets, which every user shares.
pub const X11_SOCKET_DIR: &str = "/tmp/.X11-unix";

/// The starts of the names of the compositors' sockets in the runtime directory.
const COMPOSITOR_PREFIXES: [&[u8]; 2] = [b"wayland-", b"gamescope-"];

/// The start of the names of the X servers' sockets in [`X11_SOCKET_DIR`].
const X11_PREFIXES: [&[u8]; 1] = [b"X"];

/// The user id of root.
const ROOT: u32 = 0;

/// The directories that hold display sockets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplayDirs {
    /// The user's runtime directory, which holds the compositors' sockets.
    pub runtime: PathBuf,
    /// The directory of the X servers' sockets.
    pub x11: PathBuf,
}

impl DisplayDirs {
    /// The directories of this process: the runtime directory that `XDG_RUNTIME_DIR` names, or
    /// else `/run/user/<uid>`, where systemd puts it, and [`X11_SOCKET_DIR`].
    #[must_use]
    pub fn of_this_process() -> Self {
        let runtime = crate::env::var_abspath("XDG_RUNTIME_DIR").map_or_else(
            || PathBuf::from(format!("/run/user/{}", super::uid())),
            AbsPathBuf::into_path_buf,
        );
        Self {
            runtime,
            x11: PathBuf::from(X11_SOCKET_DIR),
        }
    }

    /// The display sockets in the directories that may belong to a session of the user `uid`,
    /// with their metadata.
    ///
    /// In the runtime directory, the sockets that `uid` owns count. A socket there can be a
    /// symbolic link to a socket elsewhere, as WSLg's is, and then the metadata is the target's.
    /// In the X11 directory, the sockets that `uid` or root owns count, because a display
    /// manager or `Xorg.wrap` runs the X server of a user's session as root. Only root can
    /// create a socket that root owns, so other users can't fake one. Every user can write to
    /// that directory and could link to a socket of `uid`, so the links there don't count. A
    /// directory that can't be read has no sockets.
    pub fn sockets_for(&self, uid: u32) -> impl Iterator<Item = Metadata> {
        let compositors = sockets_in(&self.runtime, &COMPOSITOR_PREFIXES, Links::Follow)
            .filter(move |socket| socket.uid() == uid);
        let x11 = sockets_in(&self.x11, &X11_PREFIXES, Links::Skip)
            .filter(move |socket| x11_owner_counts(socket.uid(), uid));
        compositors.chain(x11)
    }
}

/// Whether an X server's socket that `owner` owns may belong to a session of the user `uid`.
const fn x11_owner_counts(owner: u32, uid: u32) -> bool {
    owner == uid || owner == ROOT
}

/// The entries of `dir` whose names start with one of `prefixes` and that are sockets, or links
/// to sockets when `links` says to follow them.
///
/// The type of an entry comes from the directory listing on most file systems, so the entries
/// of other types cost no further system call.
fn sockets_in(
    dir: &Path,
    prefixes: &'static [&'static [u8]],
    links: Links,
) -> impl Iterator<Item = Metadata> {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter(move |entry| {
            let name = entry.file_name();
            prefixes.iter().any(|prefix| name.as_encoded_bytes().starts_with(prefix))
        })
        .filter_map(move |entry| socket_metadata(&entry, links))
}

/// Whether a listing follows symbolic links.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Links {
    Follow,
    Skip,
}

/// The metadata of the socket that `entry` is, or links to when `links` says to follow it.
fn socket_metadata(entry: &DirEntry, links: Links) -> Option<Metadata> {
    let kind = entry.file_type().ok()?;
    let metadata = if kind.is_symlink() && links == Links::Follow {
        std::fs::metadata(entry.path()).ok()?
    } else if kind.is_socket() {
        entry.metadata().ok()?
    } else {
        return None;
    };
    metadata.file_type().is_socket().then_some(metadata)
}

#[cfg(test)]
mod tests {
    use std::os::unix::net::UnixListener;

    use rstest::{fixture, rstest};
    use tempfile::TempDir;

    use super::*;

    /// A runtime directory and an X11 directory, each empty, until they drop.
    struct Dirs {
        runtime: TempDir,
        x11: TempDir,
    }

    impl Dirs {
        fn display_dirs(&self) -> DisplayDirs {
            DisplayDirs {
                runtime: self.runtime.path().to_path_buf(),
                x11: self.x11.path().to_path_buf(),
            }
        }

        fn path(&self, dir: Dir, name: &str) -> PathBuf {
            match dir {
                Dir::Runtime => self.runtime.path().join(name),
                Dir::X11 => self.x11.path().join(name),
            }
        }

        fn count(&self, uid: u32) -> usize {
            self.display_dirs().sockets_for(uid).count()
        }
    }

    #[fixture]
    fn dirs() -> Dirs {
        Dirs {
            runtime: tempfile::tempdir().expect("tempdir"),
            x11: tempfile::tempdir().expect("tempdir"),
        }
    }

    /// Where a test creates a file.
    #[derive(Debug, Clone, Copy)]
    enum Dir {
        Runtime,
        X11,
    }

    /// Creates a socket at `path`, which stays on disk after the listener drops.
    fn socket(path: &Path) {
        UnixListener::bind(path).expect("bind a socket");
    }

    fn uid() -> u32 {
        super::super::uid()
    }

    #[rstest]
    #[case::wayland(Dir::Runtime, "wayland-1", 1)]
    #[case::gamescope(Dir::Runtime, "gamescope-0", 1)]
    #[case::x11(Dir::X11, "X3", 1)]
    #[case::other_socket(Dir::Runtime, "bus", 0)]
    #[case::x11_name_in_the_runtime_dir(Dir::Runtime, "X0", 0)]
    #[case::wayland_name_in_the_x11_dir(Dir::X11, "wayland-0", 0)]
    #[case::gamescope_name_in_the_x11_dir(Dir::X11, "gamescope-0", 0)]
    fn finds_the_display_sockets(
        dirs: Dirs,
        #[case] dir: Dir,
        #[case] name: &str,
        #[case] found: usize,
    ) {
        socket(&dirs.path(dir, name));

        assert_eq!(dirs.count(uid()), found);
    }

    /// A compositor's lock file is a regular file, and only the socket next to it counts.
    #[rstest]
    fn a_lock_file_is_no_socket(dirs: Dirs) {
        std::fs::write(dirs.path(Dir::Runtime, "wayland-0.lock"), b"").expect("write");

        assert_eq!(dirs.count(uid()), 0);
    }

    #[rstest]
    fn a_link_to_a_socket_counts_as_the_socket(dirs: Dirs) {
        let target = dirs.path(Dir::X11, "elsewhere");
        socket(&target);
        std::os::unix::fs::symlink(&target, dirs.path(Dir::Runtime, "wayland-0")).expect("link");

        assert_eq!(dirs.count(uid()), 1);
    }

    /// Every user can write to the X11 directory, so a link there to a socket of this user
    /// doesn't count.
    #[rstest]
    fn a_link_in_the_x11_dir_does_not_count(dirs: Dirs) {
        let target = dirs.path(Dir::Runtime, "agent");
        socket(&target);
        std::os::unix::fs::symlink(&target, dirs.path(Dir::X11, "X42")).expect("link");

        assert_eq!(dirs.count(uid()), 0);
    }

    /// The sockets of another user count in neither directory. The test runs as a user other
    /// than root, as a test of the user's own sockets must.
    #[rstest]
    fn the_sockets_of_another_user_do_not_count(
        dirs: Dirs,
        #[values(Dir::Runtime, Dir::X11)] dir: Dir,
    ) {
        socket(&dirs.path(dir, "X5"));
        socket(&dirs.path(dir, "wayland-5"));

        assert_eq!(dirs.count(uid() + 1), 0);
    }

    /// The X server of a session may run as root, and no other user's X server counts.
    #[rstest]
    #[case::the_user(1000, true)]
    #[case::root(ROOT, true)]
    #[case::another_user(1001, false)]
    fn x11_sockets_of_the_user_and_of_root_count(#[case] owner: u32, #[case] counts: bool) {
        assert_eq!(x11_owner_counts(owner, 1000), counts);
    }

    #[rstest]
    fn missing_directories_have_no_sockets() {
        let dirs = DisplayDirs {
            runtime: PathBuf::from("/nonexistent/runtime"),
            x11: PathBuf::from("/nonexistent/x11"),
        };

        assert_eq!(dirs.sockets_for(uid()).count(), 0);
    }
}
