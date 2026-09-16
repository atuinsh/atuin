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
}

impl Disk {
    /// Get information on the disk behind this path.
    pub fn of_path(path: &Path) -> io::Result<Self> {
        #[cfg(unix)]
        {
            Ok(Self {
                total: unix::disk::total_space(path)?,
            })
        }

        #[cfg(windows)]
        {
            Ok(Self {
                total: windows::disk::total_space(path)?,
            })
        }
    }

    #[must_use]
    /// How much total space does the disk behind `path` have.
    pub fn total_space(&self) -> ByteSize {
        self.total
    }
}
