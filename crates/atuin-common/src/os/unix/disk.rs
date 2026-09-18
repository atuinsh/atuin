use std::io;
use std::path::Path;

use rustix::fs;

use crate::units::ByteSize;

/// How much total space does the disk behind `path` have.
pub fn total_space(path: &Path) -> io::Result<ByteSize> {
    // `statfs` reports 64-bit block counts but is unavailable on the solarish
    // family; `statvfs` is portable but truncates the count to a u32 on macOS.
    // Prefer `statfs` where it is both present and wide, `statvfs` elsewhere.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        let stat = fs::statfs(path)?;

        #[cfg(target_os = "linux")]
        let block_size = u64::try_from(stat.f_bsize).unwrap_or(0);

        #[cfg(target_os = "macos")]
        let block_size = u64::from(stat.f_bsize);

        Ok(ByteSize::b(block_size.saturating_mul(stat.f_blocks)))
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let stat = fs::statvfs(path)?;
        Ok(ByteSize::b(stat.f_frsize.saturating_mul(stat.f_blocks)))
    }
}
