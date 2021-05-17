use std::cmp::Ordering;
use std::ffi::CString;
use std::os::raw::{c_int, c_void};
use std::slice;
use std::str::from_utf8_unchecked;

use libsqlite3_sys::{sqlite3_create_collation_v2, SQLITE_OK, SQLITE_UTF8};

use crate::error::Error;
use crate::sqlite::connection::handle::ConnectionHandle;
use crate::sqlite::SqliteError;

unsafe extern "C" fn free_boxed_value<T>(p: *mut c_void) {
    drop(Box::from_raw(p as *mut T));
}

pub(crate) fn create_collation<F>(
    handle: &ConnectionHandle,
    name: &str,
    compare: F,
) -> Result<(), Error>
where
    F: Fn(&str, &str) -> Ordering + Send + Sync + 'static,
{
    unsafe extern "C" fn call_boxed_closure<C>(
        arg1: *mut c_void,
        arg2: c_int,
        arg3: *const c_void,
        arg4: c_int,
        arg5: *const c_void,
    ) -> c_int
    where
        C: Fn(&str, &str) -> Ordering,
    {
        let boxed_f: *mut C = arg1 as *mut C;
        debug_assert!(!boxed_f.is_null());
        let s1 = {
            let c_slice = slice::from_raw_parts(arg3 as *const u8, arg2 as usize);
            from_utf8_unchecked(c_slice)
        };
        let s2 = {
            let c_slice = slice::from_raw_parts(arg5 as *const u8, arg4 as usize);
            from_utf8_unchecked(c_slice)
        };
        let t = (*boxed_f)(s1, s2);

        match t {
            Ordering::Less => -1,
            Ordering::Equal => 0,
            Ordering::Greater => 1,
        }
    }

    let boxed_f: *mut F = Box::into_raw(Box::new(compare));
    let c_name =
        CString::new(name).map_err(|_| err_protocol!("invalid collation name: {}", name))?;
    let flags = SQLITE_UTF8;
    let r = unsafe {
        sqlite3_create_collation_v2(
            handle.as_ptr(),
            c_name.as_ptr(),
            flags,
            boxed_f as *mut c_void,
            Some(call_boxed_closure::<F>),
            Some(free_boxed_value::<F>),
        )
    };

    if r == SQLITE_OK {
        Ok(())
    } else {
        // The xDestroy callback is not called if the sqlite3_create_collation_v2() function fails.
        drop(unsafe { Box::from_raw(boxed_f) });
        Err(Error::Database(Box::new(SqliteError::new(handle.as_ptr()))))
    }
}
