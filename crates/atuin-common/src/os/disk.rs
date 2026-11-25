use std::io;
use std::path::Path;

#[cfg(unix)]
use crate::os::unix;
#[cfg(windows)]
use crate::os::windows;
use crate::units::ByteSize;

#[derive(Debug, Clone, Copy)]
pub struct Disk {
    total: ByteSize,
    available: ByteSize,
}

impl Disk {
    /// Get information on the disk behind this path.
    pub fn of_path(path: &Path) -> io::Result<Self> {
        #[cfg(unix)]
        {
            unix::disk::of_path(path)
        }

        #[cfg(windows)]
        {
            windows::disk::of_path(path)
        }
    }

    pub(crate) const fn new(total: ByteSize, available: ByteSize) -> Self {
        Self { total, available }
    }

    #[must_use]
    /// How much total space does the disk behind `path` have.
    pub fn total_space(&self) -> ByteSize {
        self.total
    }

    /// How much space the disk behind `path` has free for the current user.
    #[must_use]
    pub fn available_space(&self) -> ByteSize {
        self.available
    }
}
