//! Unix-specific utilities.

use std::path::{Path, PathBuf};

use rustix::fs;

/// Get the current UID.
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
    #[error(transparent)]
    Io(#[from] std::io::Error),
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
pub fn create_secure_temp_dir<P>(path: P) -> Result<P, SecureTempDirError>
where
    P: AsRef<Path> + Into<PathBuf>,
{
    use std::io::ErrorKind;
    use std::os::unix::fs::{DirBuilderExt, MetadataExt};

    match std::fs::DirBuilder::new().mode(0o700).create(path.as_ref()) {
        Ok(()) => return Ok(path),
        Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e.into()),
    }

    // Make sure we own the directory with the appropriate permissions. Otherwise, another user
    // on the system could access the files we store in the directory.

    let meta = fs_err::symlink_metadata(path.as_ref())?;
    if !meta.is_dir() {
        // This importantly rejects symlinks; a symlink could point to a directory owned by
        // another user, who could then access our files.
        return Err(SecureTempDirError::NotADirectory(path.into()));
    }

    let expected_uid = uid();
    let actual_uid = meta.uid();
    if !std::ffi::c_uint::try_from(actual_uid).is_ok_and(|actual| actual == expected_uid) {
        // Reject the directory if it's owned by another user.
        return Err(SecureTempDirError::WrongOwner {
            path: path.into(),
            expected_uid,
            actual_uid,
        });
    }

    let permissions = meta.mode() & 0o777;
    if permissions & 0o077 != 0 {
        // Reject the directory if it is accessible by others. On some systems, if a socket gets
        // created in the directory, even read permission on the directory could allow another user
        // to connect to the socket, who could then interfere with our connection.
        return Err(SecureTempDirError::WrongPermissions {
            path: path.into(),
            permissions,
        });
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::{MetadataExt, symlink};
    use std::os::unix::net::UnixListener;

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

    #[test]
    fn touching_a_socket_refreshes_its_timestamps() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("atuin.sock");
        let _listener = UnixListener::bind(&path).unwrap();
        backdate(&path);
        touch_file(&path).unwrap();
        assert!(fs_err::symlink_metadata(&path).unwrap().mtime() > 0);
    }

    #[test]
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
