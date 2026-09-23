//! Unix-specific utilities.

pub mod disk;
pub mod io;
pub mod process;
pub mod tty;

use std::path::{Path, PathBuf};

use rustix::fs;

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

/// Get the global temporary directory.
pub fn tmp_dir() -> PathBuf {
    // TODO: We should perhaps use `std::env::temp_dir()` instead, but that would be a breaking
    // change and could cause clients to fail to connect to an older running daemon.
    crate::utils::env_nonempty("TMPDIR").map_or_else(|| "/tmp".into(), Into::into)
}

/// Error returned by [`create_secure_temp_dir`].
#[derive(Debug, thiserror::Error)]
pub enum SecureTempDirError {
    #[error("{} is not a directory", .0.display())]
    NotADirectory(PathBuf),
    #[error(
        "{} is not owned by the current user (expected uid {expected_uid}, got {actual_uid})",
        .path.display(),
    )]
    WrongOwner {
        path: PathBuf,
        expected_uid: std::ffi::c_uint,
        actual_uid: u32,
    },
    #[error("{} has incorrect permissions (expected 700, got {permissions:03o})", .path.display())]
    WrongPermissions {
        path: PathBuf,
        permissions: u32,
    },
    #[error("could not set up {}: {source}", .path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

impl SecureTempDirError {
    /// An [`Self::Io`] for `path`, for `map_err`.
    fn io(path: &Path) -> impl FnOnce(std::io::Error) -> Self {
        move |source| Self::Io {
            path: path.to_owned(),
            source,
        }
    }

    /// Check that the existing directory at `path`, described by `meta`, is private to this user.
    ///
    /// `meta` must come from `symlink_metadata` so that a symlink is rejected rather than followed.
    fn ensure_private<P: Into<PathBuf>>(path: P, meta: &std::fs::Metadata) -> Result<P, Self> {
        use std::os::unix::fs::MetadataExt;

        if !meta.is_dir() {
            // This importantly rejects symlinks; a symlink could point to a directory owned by
            // another user, who could then access our files.
            return Err(Self::NotADirectory(path.into()));
        }

        let expected_uid = uid();
        let actual_uid = meta.uid();
        if !std::ffi::c_uint::try_from(actual_uid).is_ok_and(|actual| actual == expected_uid) {
            return Err(Self::WrongOwner {
                path: path.into(),
                expected_uid,
                actual_uid,
            });
        }

        let permissions = meta.mode() & 0o777;
        if permissions & 0o077 != 0 {
            // On some systems, if a socket gets created in the directory, even read permission on
            // the directory could allow another user to connect to the socket, who could then
            // interfere with our connection.
            return Err(Self::WrongPermissions {
                path: path.into(),
                permissions,
            });
        }
        Ok(path)
    }
}

/// Create a secure temporary directory with the given path.
///
/// Generally, `path` will be a subdirectory of `/tmp`.
///
/// Every component of the path except the last must exist: this function does not create ancestors.
///
/// This function ensures the directory is owned by the current user with appropriate permissions to
/// prevent other users from accessing its contents. This is especially important for sockets --
/// some systems ignore permissions on sockets themselves and allow any user who can access the
/// socket file to connect to it.
///
/// On success, returns `path`. This may allow resources to be reused if `P` is an owned type.
pub async fn create_secure_temp_dir<P>(path: P) -> Result<P, SecureTempDirError>
where
    P: AsRef<Path> + Into<PathBuf>,
{
    match crate::fs::create_secure_dir(path.as_ref(), 0o700).await {
        Ok(()) => return Ok(path),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(SecureTempDirError::io(path.as_ref())(e)),
    }
    let meta = crate::fs::symlink_metadata(path.as_ref())
        .await
        .map_err(SecureTempDirError::io(path.as_ref()))?;
    SecureTempDirError::ensure_private(path, &meta)
}

/// Equivalent to [`create_secure_temp_dir`], except it blocks the calling thread.
///
/// Only for threads with no tokio runtime, such as the PTY proxy's.
pub fn create_secure_temp_dir_blocking<P>(path: P) -> Result<P, SecureTempDirError>
where
    P: AsRef<Path> + Into<PathBuf>,
{
    match crate::fs::blocking::create_secure_dir(path.as_ref(), 0o700) {
        Ok(()) => return Ok(path),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(SecureTempDirError::io(path.as_ref())(e)),
    }
    let meta = crate::fs::blocking::symlink_metadata(path.as_ref())
        .map_err(SecureTempDirError::io(path.as_ref()))?;
    SecureTempDirError::ensure_private(path, &meta)
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

    #[rstest]
    fn a_secure_temp_dir_io_error_names_its_path() {
        let tmp = tempfile::tempdir().unwrap();
        let orphan = tmp.path().join("missing/dir");
        let Err(SecureTempDirError::Io { path, source }) =
            create_secure_temp_dir_blocking(orphan.clone())
        else {
            panic!("a directory under a missing parent cannot be created");
        };
        assert_eq!((path, source.kind()), (orphan, std::io::ErrorKind::NotFound));
    }
}
