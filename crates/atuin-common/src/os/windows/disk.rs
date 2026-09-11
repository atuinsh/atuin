#![allow(unsafe_code)]

use std::io;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;

use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

use super::fallible_do;
use crate::units::ByteSize;

pub fn total_space(path: &Path) -> io::Result<ByteSize> {
    let directory: Vec<u16> = path.as_os_str().encode_wide().chain(std::iter::once(0)).collect();

    let mut total: u64 = 0;
    fallible_do(|| unsafe {
        GetDiskFreeSpaceExW(
            directory.as_ptr(),
            std::ptr::null_mut(),
            &mut total,
            std::ptr::null_mut(),
        )
    })?;

    Ok(ByteSize::b(total))
}
