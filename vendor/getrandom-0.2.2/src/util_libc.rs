// Copyright 2019 Developers of the Rand project.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// https://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.
#![allow(dead_code)]
use crate::{util::LazyUsize, Error};
use core::{num::NonZeroU32, ptr::NonNull};

cfg_if! {
    if #[cfg(any(target_os = "netbsd", target_os = "openbsd", target_os = "android"))] {
        use libc::__errno as errno_location;
    } else if #[cfg(any(target_os = "linux", target_os = "emscripten", target_os = "redox"))] {
        use libc::__errno_location as errno_location;
    } else if #[cfg(any(target_os = "solaris", target_os = "illumos"))] {
        use libc::___errno as errno_location;
    } else if #[cfg(any(target_os = "macos", target_os = "freebsd"))] {
        use libc::__error as errno_location;
    } else if #[cfg(target_os = "haiku")] {
        use libc::_errnop as errno_location;
    }
}

cfg_if! {
    if #[cfg(target_os = "vxworks")] {
        use libc::errnoGet as get_errno;
    } else if #[cfg(target_os = "dragonfly")] {
        // Until rust-lang/rust#29594 is stable, we cannot get the errno value
        // on DragonFlyBSD. So we just return an out-of-range errno.
        unsafe fn get_errno() -> libc::c_int { -1 }
    } else {
        unsafe fn get_errno() -> libc::c_int { *errno_location() }
    }
}

pub fn last_os_error() -> Error {
    let errno = unsafe { get_errno() };
    if errno > 0 {
        Error::from(NonZeroU32::new(errno as u32).unwrap())
    } else {
        Error::ERRNO_NOT_POSITIVE
    }
}

// Fill a buffer by repeatedly invoking a system call. The `sys_fill` function:
//   - should return -1 and set errno on failure
//   - should return the number of bytes written on success
pub fn sys_fill_exact(
    mut buf: &mut [u8],
    sys_fill: impl Fn(&mut [u8]) -> libc::ssize_t,
) -> Result<(), Error> {
    while !buf.is_empty() {
        let res = sys_fill(buf);
        if res < 0 {
            let err = last_os_error();
            // We should try again if the call was interrupted.
            if err.raw_os_error() != Some(libc::EINTR) {
                return Err(err);
            }
        } else {
            // We don't check for EOF (ret = 0) as the data we are reading
            // should be an infinite stream of random bytes.
            buf = &mut buf[(res as usize)..];
        }
    }
    Ok(())
}

// A "weak" binding to a C function that may or may not be present at runtime.
// Used for supporting newer OS features while still building on older systems.
// F must be a function pointer of type `unsafe extern "C" fn`. Based off of the
// weak! macro in libstd.
pub struct Weak {
    name: &'static str,
    addr: LazyUsize,
}

impl Weak {
    // Construct a binding to a C function with a given name. This function is
    // unsafe because `name` _must_ be null terminated.
    pub const unsafe fn new(name: &'static str) -> Self {
        Self {
            name,
            addr: LazyUsize::new(),
        }
    }

    // Return a function pointer if present at runtime. Otherwise, return null.
    pub fn ptr(&self) -> Option<NonNull<libc::c_void>> {
        let addr = self.addr.unsync_init(|| unsafe {
            libc::dlsym(libc::RTLD_DEFAULT, self.name.as_ptr() as *const _) as usize
        });
        NonNull::new(addr as *mut _)
    }
}

cfg_if! {
    if #[cfg(any(target_os = "linux", target_os = "emscripten"))] {
        use libc::open64 as open;
    } else {
        use libc::open;
    }
}

// SAFETY: path must be null terminated, FD must be manually closed.
pub unsafe fn open_readonly(path: &str) -> Result<libc::c_int, Error> {
    debug_assert_eq!(path.as_bytes().last(), Some(&0));
    let fd = open(path.as_ptr() as *const _, libc::O_RDONLY | libc::O_CLOEXEC);
    if fd < 0 {
        return Err(last_os_error());
    }
    Ok(fd)
}
