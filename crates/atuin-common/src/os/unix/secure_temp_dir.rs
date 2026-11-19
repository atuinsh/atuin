use std::path::{Path, PathBuf};

use super::{socket_in_use, uid};

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
pub fn create_secure_temp_dir(path: &Path) -> Result<(), SecureTempDirError> {
    match create_secure_temp_dir_once(path) {
        // On macOS, Atuin's launchd agent used to have launchd create the socket. launchd creates
        // the socket's parent directory as root, even when `$TMPDIR` itself is user-owned, causing
        // the ownership check here to fail. When this happens, try renaming the root-owned dir,
        // which we can do if `$TMPDIR` is user-owned.
        Err(err @ SecureTempDirError::WrongOwner { actual_uid: 0, .. })
            if cfg!(target_os = "macos") =>
        {
            replace_root_owned_dir(path, err)
        }
        checked => checked,
    }
}

/// The core of [`create_secure_temp_dir`]. Doesn't try [moving a root-owned directory aside][0]
/// when the ownership check fails.
///
/// [0]: replace_root_owned_dir
fn create_secure_temp_dir_once(path: &Path) -> Result<(), SecureTempDirError> {
    use std::io::ErrorKind;
    use std::os::unix::fs::DirBuilderExt;

    match std::fs::DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == ErrorKind::AlreadyExists => ensure_private(path),
        Err(e) => Err(e.into()),
    }
}

/// Make sure that `path` is a directory that only the current user can open. Otherwise, another
/// user on the system could access the files we store in the directory.
fn ensure_private(path: &Path) -> Result<(), SecureTempDirError> {
    use std::os::unix::fs::MetadataExt;

    let meta = fs_err::symlink_metadata(path)?;
    if !meta.is_dir() {
        // This importantly rejects symlinks; a symlink could point to a directory owned by
        // another user, who could then access our files.
        return Err(SecureTempDirError::NotADirectory(path.to_owned()));
    }

    let expected_uid = uid();
    let actual_uid = meta.uid();
    if !std::ffi::c_uint::try_from(actual_uid).is_ok_and(|actual| actual == expected_uid) {
        // Reject the directory if it's owned by another user.
        return Err(SecureTempDirError::WrongOwner {
            path: path.to_owned(),
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
            path: path.to_owned(),
            permissions,
        });
    }
    Ok(())
}

/// Move the root-owned directory `dir` aside and create it again,
///
/// This is done only if the directory's parent is [private](ensure_private), to ensure no other
/// user could have planted files or symlinks there.
fn replace_root_owned_dir(dir: &Path, err: SecureTempDirError) -> Result<(), SecureTempDirError> {
    let Some(_lock) = dir.parent().and_then(lock_directory_renames) else {
        return Err(err);
    };
    match create_secure_temp_dir_once(dir) {
        Err(SecureTempDirError::WrongOwner { actual_uid: 0, .. }) => {
            let _ = move_aside_root_owned_dir(dir);
            create_secure_temp_dir_once(dir)
        }
        checked => checked,
    }
}

/// Lock the file `.atuin-directory-rename-<UID>.lock` in `parent` to ensure
/// [`move_aside_root_owned_dir`] only runs in one process at a time.
fn lock_directory_renames(parent: &Path) -> Option<std::fs::File> {
    use std::fmt::Write as _;

    use crate::fs::lock::{LockMode, LockOptions};

    ensure_private(parent).ok()?;
    let mut path = parent.join(".atuin-directory-rename-");
    write!(path.as_mut_os_string(), "{}.lock", uid()).expect("writing to an OsString can't fail");
    LockOptions {
        create: true,
        mode: LockMode::Exclusive,
    }
    .open(&path)
    .inspect_err(|err| tracing::warn!("could not lock {}: {err}", path.display()))
    .ok()
}

/// Move `dir`, which root owns, out of the way. The caller should hold the lock from
/// [`lock_directory_renames`].
///
/// The directory is moved only if its parent is [private](ensure_private). `$TMPDIR` on macOS
/// satisfies this condition.
///
/// A directory that holds a listening socket is left alone. Moving it would take the socket's path
/// away from its clients.
///
/// The directory is renamed but remains in `parent`.
fn move_aside_root_owned_dir(dir: &Path) -> Result<(), ()> {
    use std::fmt::Write as _;
    use std::os::unix::fs::FileTypeExt as _;

    let (Some(parent), Some(name)) = (dir.parent(), dir.file_name()) else {
        return Err(());
    };

    let in_use = || {
        std::fs::read_dir(dir).map_or(true, |entries| {
            entries.flatten().any(|entry| {
                entry.file_type().is_ok_and(|kind| kind.is_socket())
                    && !matches!(socket_in_use(&entry.path()), Ok(false))
            })
        })
    };
    if ensure_private(parent).is_err() || in_use() {
        return Err(());
    }

    let mut aside = parent.to_owned();
    aside.push(name);
    write!(aside.as_mut_os_string(), ".root-owned.{}", std::process::id())
        .expect("writing to an OsString can't fail");

    match std::fs::rename(dir, &aside) {
        Ok(()) => {
            tracing::warn!(
                "moved root-owned directory {} to {} to avoid conflicts with atuin",
                dir.display(),
                aside.display()
            );
            Ok(())
        }
        // It's gone already.
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => {
            tracing::warn!("could not rename {}: {err}", dir.display());
            Err(())
        }
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::net::UnixListener;

    use rstest::rstest;

    use super::*;

    /// What the directory under test holds.
    #[derive(Debug, Clone, Copy)]
    enum Holds {
        Nothing,
        /// A socket that a daemon left behind when it stopped.
        StaleSocket,
        ListeningSocket,
    }

    /// A test can't make root the owner of a directory. These directories belong to the user who
    /// runs the test. The parent's mode and the directory's contents decide the result.
    #[rstest]
    #[case::empty_in_a_private_parent(0o700, Holds::Nothing, true)]
    #[case::stale_socket(0o700, Holds::StaleSocket, true)]
    #[case::listening_socket(0o700, Holds::ListeningSocket, false)]
    #[case::parent_others_can_open(0o755, Holds::Nothing, false)]
    #[case::parent_group_can_write(0o770, Holds::Nothing, false)]
    fn a_directory_is_moved_aside_only_from_a_private_parent_and_unused(
        #[case] parent_mode: u32,
        #[case] holds: Holds,
        #[case] moved: bool,
    ) {
        use std::os::unix::fs::PermissionsExt as _;

        let parent = tempfile::tempdir().unwrap();
        let dir = parent.path().join("atuin-1234");
        fs_err::create_dir(&dir).unwrap();
        let _listener = match holds {
            Holds::Nothing => None,
            Holds::StaleSocket => {
                drop(UnixListener::bind(dir.join("atuin.sock")).unwrap());
                None
            }
            Holds::ListeningSocket => Some(UnixListener::bind(dir.join("atuin.sock")).unwrap()),
        };
        fs_err::set_permissions(parent.path(), std::fs::Permissions::from_mode(parent_mode))
            .unwrap();

        assert_eq!(move_aside_root_owned_dir(&dir).is_ok(), moved);
        assert_eq!(dir.exists(), !moved);
        let aside = parent.path().join(format!("atuin-1234.root-owned.{}", std::process::id()));
        assert_eq!(aside.is_dir(), moved);
    }

    /// The lock file is created only in a private parent. No lock file is ever created in a shared
    /// directory such as `/tmp`.
    #[rstest]
    #[case::private_parent(0o700, true)]
    #[case::parent_others_can_open(0o755, false)]
    fn the_lock_is_taken_only_in_a_private_parent(#[case] parent_mode: u32, #[case] locked: bool) {
        use std::os::unix::fs::PermissionsExt as _;

        let parent = tempfile::tempdir().unwrap();
        fs_err::set_permissions(parent.path(), std::fs::Permissions::from_mode(parent_mode))
            .unwrap();
        let lock_file = parent.path().join(format!(".atuin-directory-rename-{}.lock", uid()));

        let lock = lock_directory_renames(parent.path());

        assert_eq!(lock.is_some(), locked);
        assert_eq!(lock_file.exists(), locked);
    }

    /// While one process holds the lock, another can't take it.
    #[rstest]
    fn a_held_lock_makes_the_next_taker_wait() {
        use std::os::unix::fs::PermissionsExt as _;

        let parent = tempfile::tempdir().unwrap();
        fs_err::set_permissions(parent.path(), std::fs::Permissions::from_mode(0o700)).unwrap();

        let held = lock_directory_renames(parent.path()).unwrap();
        // Opening the file again gives a separate open file description. Another process would
        // have its own one too.
        let lock_file = parent.path().join(format!(".atuin-directory-rename-{}.lock", uid()));
        let other = fs_err::File::open(lock_file).unwrap();
        assert!(matches!(other.file().try_lock(), Err(std::fs::TryLockError::WouldBlock)));

        drop(held);
        other.file().try_lock().unwrap();
    }
}
