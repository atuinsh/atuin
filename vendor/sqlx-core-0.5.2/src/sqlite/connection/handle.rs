use std::ptr::NonNull;

use libsqlite3_sys::{sqlite3, sqlite3_close, SQLITE_OK};

use crate::sqlite::SqliteError;

/// Managed handle to the raw SQLite3 database handle.
/// The database handle will be closed when this is dropped.
#[derive(Debug)]
pub(crate) struct ConnectionHandle(pub(super) NonNull<sqlite3>);

// A SQLite3 handle is safe to send between threads, provided not more than
// one is accessing it at the same time. This is upheld as long as [SQLITE_CONFIG_MULTITHREAD] is
// enabled and [SQLITE_THREADSAFE] was enabled when sqlite was compiled. We refuse to work
// if these conditions are not upheld.

// <https://www.sqlite.org/c3ref/threadsafe.html>

// <https://www.sqlite.org/c3ref/c_config_covering_index_scan.html#sqliteconfigmultithread>

unsafe impl Send for ConnectionHandle {}

impl ConnectionHandle {
    #[inline]
    pub(super) unsafe fn new(ptr: *mut sqlite3) -> Self {
        Self(NonNull::new_unchecked(ptr))
    }

    #[inline]
    pub(crate) fn as_ptr(&self) -> *mut sqlite3 {
        self.0.as_ptr()
    }
}

impl Drop for ConnectionHandle {
    fn drop(&mut self) {
        unsafe {
            // https://sqlite.org/c3ref/close.html
            let status = sqlite3_close(self.0.as_ptr());
            if status != SQLITE_OK {
                // this should *only* happen due to an internal bug in SQLite where we left
                // SQLite handles open
                panic!("{}", SqliteError::new(self.0.as_ptr()));
            }
        }
    }
}
