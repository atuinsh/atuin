#![allow(unsafe_code)]

use std::io;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;

use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

use super::fallible_do;
use crate::os::disk::Disk;
use crate::units::ByteSize;

/// The total space of the disk behind `path`, and the space that the disk has free for the
/// current user.
pub fn of_path(path: &Path) -> io::Result<Disk> {
    let directory: Vec<u16> = path.as_os_str().encode_wide().chain(std::iter::once(0)).collect();

    let mut available: u64 = 0;
    let mut total: u64 = 0;
    fallible_do(|| unsafe {
        // Classic win32 misnomer -- this returns both the total and available space.
        GetDiskFreeSpaceExW(directory.as_ptr(), &mut available, &mut total, std::ptr::null_mut())
    })?;

    Ok(Disk::new(ByteSize::b(total), ByteSize::b(available)))
}
