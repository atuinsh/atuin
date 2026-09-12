use std::io;
use std::path::Path;

use rustix::fs;

use crate::units::ByteSize;

/// How much total space does the disk behind `path` have.
pub fn total_space(path: &Path) -> io::Result<ByteSize> {
    let stat = fs::statvfs(path)?;
    Ok(ByteSize::b(stat.f_frsize.saturating_mul(stat.f_blocks)))
}
