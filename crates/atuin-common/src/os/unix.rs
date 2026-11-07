//! Unix-specific utilities.

use std::path::Path;

use rustix::fs;

use crate::path::AbsPathBuf;

pub mod disk;
pub mod io;
pub mod process;
pub mod tty;

mod secure_temp_dir;
mod socket;
pub use secure_temp_dir::{SecureTempDirError, create_secure_temp_dir};
pub use socket::socket_in_use;

/// Get the current UID.
#[must_use]
pub fn uid() -> std::ffi::c_uint {
    rustix::process::getuid().as_raw()
}

/// Update a file's modification time to the current time.
///
/// This function does *not* follow symlinks.
pub fn touch_file(path: &Path) -> std::io::Result<()> {
    let now = fs::Timespec {
        tv_sec: 0,
        tv_nsec: fs::UTIME_NOW,
    };
    fs::utimensat(
        fs::CWD,
        path,
        &fs::Timestamps {
            last_access: now,
            last_modification: now,
        },
        fs::AtFlags::SYMLINK_NOFOLLOW,
    )?;
    Ok(())
}

/// Get the global temporary directory: `$TMPDIR`, or `/tmp` if that is unset or relative.
#[must_use]
pub fn tmp_dir() -> AbsPathBuf {
    // TODO: We should perhaps use `std::env::temp_dir()` instead, but that would be a breaking
    // change and could cause clients to fail to connect to an older running daemon.
    crate::env::var_abspath("TMPDIR")
        .unwrap_or_else(|| AbsPathBuf::new("/tmp").expect("/tmp is absolute"))
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::{MetadataExt, symlink};
    use std::os::unix::net::UnixListener;

    use rstest::rstest;

    use super::*;

    /// Set a file's timestamp to the Unix epoch.
    fn backdate(path: &Path) {
        let epoch = fs::Timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        fs::utimensat(
            fs::CWD,
            path,
            &fs::Timestamps {
                last_access: epoch,
                last_modification: epoch,
            },
            fs::AtFlags::SYMLINK_NOFOLLOW,
        )
        .unwrap();
        assert_eq!(fs_err::symlink_metadata(path).unwrap().mtime(), 0);
    }

    #[rstest]
    fn touching_a_socket_refreshes_its_timestamps() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("atuin.sock");
        let _listener = UnixListener::bind(&path).unwrap();
        backdate(&path);
        touch_file(&path).unwrap();
        assert!(fs_err::symlink_metadata(&path).unwrap().mtime() > 0);
    }

    #[rstest]
    fn touching_a_symlink_leaves_its_target_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let (target, link) = (tmp.path().join("target"), tmp.path().join("link"));
        fs_err::File::create(&target).unwrap();
        symlink(&target, &link).unwrap();
        backdate(&target);
        backdate(&link);
        touch_file(&link).unwrap();
        assert!(fs_err::symlink_metadata(&link).unwrap().mtime() > 0);
        assert_eq!(fs_err::symlink_metadata(&target).unwrap().mtime(), 0);
    }
}
