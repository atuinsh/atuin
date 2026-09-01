//! Windows-specific utilities.
//!
//! There are some crates that could handle this for us, such as `winsafe`, but they're large
//! dependencies for a relatively small problem.
#![allow(unsafe_code, reason = "win32 API calls all require unsafe.")]

use windows_sys::Win32::Foundation::GetLastError;

pub mod process;

/// Query the system for the last set error.
#[must_use]
pub fn get_last_error() -> std::io::Error {
    // Wrapping behavior of `as` is intended here -- reinterpreting `DWORD` Windows error codes as
    // `i32` is what `std::io::Error` expects.
    std::io::Error::from_raw_os_error(unsafe { GetLastError() } as i32)
}

/// Perform a windows operation that returns a `BOOL`-like status.
///
/// A returned value of `0` is treated as the failure case, matching the Win32 convention where a
/// zero `BOOL` signals failure and the reason is retrieved via [`GetLastError`].
pub fn fallible_do<T, F>(op: F) -> Result<(), std::io::Error>
where
    F: FnOnce() -> T,
    T: PartialEq<i32>,
{
    if op() == 0 {
        Err(get_last_error())
    } else {
        Ok(())
    }
}
