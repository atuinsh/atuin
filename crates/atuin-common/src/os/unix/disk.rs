use std::io;
use std::path::Path;

use rustix::fs;

use crate::units::ByteSize;

/// How much total space does the disk behind `path` have.
pub fn total_space(path: &Path) -> io::Result<ByteSize> {
    // On macOS, `statvfs` truncates the block count to a u32.
    //
    // `statfs` reports u64(ish) across all platforms.
    let stat = fs::statfs(path)?;

    #[cfg(target_os = "linux")]
    let block_size = u64::try_from(stat.f_bsize).unwrap_or(0);

    #[cfg(target_os = "macos")]
    let block_size = u64::from(stat.f_bsize);

    Ok(ByteSize::b(block_size.saturating_mul(stat.f_blocks)))
}
