//! Generic windows-specific utilities.

pub mod process;

/// Query the system for the last set error.
pub fn get_last_error() -> std::io::Error {
    std::io::Error::from_raw_os_error(unsafe { GetLastError() })
}

/// Perform a windows operation that may or may not fail.
///
/// The value `0` represents the fallible case. This makes this function usable with functions that
/// return either `BOOL` or pointers.
pub fn fallible_do<T, F>(op: F) -> Result<(), std::io::Error>
where
    F: FnOnce() -> T,
{
    if op() == 0 {
        Err(get_last_error())
    } else {
        Ok(())
    }
}
