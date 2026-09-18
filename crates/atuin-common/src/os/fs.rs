//! Cross-platform file identity.

use std::io;

/// A stable identity for an open file, equal iff two handles refer to the same file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FdIdentity {
    device: u64,
    inode: u64,
}

impl FdIdentity {
    #[cfg(test)]
    pub(crate) fn from_raw(device: u64, inode: u64) -> Self {
        Self { device, inode }
    }
}

/// Exposes the [`FdIdentity`] of any file handle.
#[cfg(unix)]
pub trait FdIdentityExt: std::os::fd::AsFd {
    /// The identity of the file this handle refers to.
    fn identity(&self) -> io::Result<FdIdentity> {
        use std::os::unix::fs::MetadataExt;

        let owned = self.as_fd().try_clone_to_owned()?;
        let meta = std::fs::File::from(owned).metadata()?;
        Ok(FdIdentity {
            device: meta.dev(),
            inode: meta.ino(),
        })
    }
}

#[cfg(unix)]
impl<T: std::os::fd::AsFd + ?Sized> FdIdentityExt for T {}

/// Exposes the [`FdIdentity`] of any file handle.
#[cfg(windows)]
pub trait FdIdentityExt: std::os::windows::io::AsHandle {
    /// The identity of the file this handle refers to.
    fn identity(&self) -> io::Result<FdIdentity> {
        use std::os::windows::io::AsRawHandle;

        use windows_sys::Win32::Storage::FileSystem::{
            BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
        };

        let handle = self.as_handle().as_raw_handle();
        #[allow(unsafe_code, reason = "win32 GetFileInformationByHandle FFI")]
        let info = unsafe {
            let mut info: BY_HANDLE_FILE_INFORMATION = std::mem::zeroed();
            if GetFileInformationByHandle(handle.cast(), &mut info) == 0 {
                return Err(io::Error::last_os_error());
            }
            info
        };
        Ok(FdIdentity {
            device: u64::from(info.dwVolumeSerialNumber),
            inode: (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
        })
    }
}

#[cfg(windows)]
impl<T: std::os::windows::io::AsHandle + ?Sized> FdIdentityExt for T {}

#[cfg(all(test, unix))]
mod tests {
    use std::fs::File;

    use super::*;

    #[test]
    fn distinct_files_have_distinct_identities() {
        let dir = tempfile::tempdir().unwrap();
        let a = File::create(dir.path().join("a")).unwrap();
        let b = File::create(dir.path().join("b")).unwrap();
        assert_ne!(a.identity().unwrap(), b.identity().unwrap());
    }

    #[test]
    fn the_same_file_keeps_its_identity_across_handles() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f");
        let created = File::create(&path).unwrap();
        let reopened = File::open(&path).unwrap();
        assert_eq!(created.identity().unwrap(), reopened.identity().unwrap());
    }
}
