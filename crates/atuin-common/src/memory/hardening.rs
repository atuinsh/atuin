//! Platform-specific hardening for a [`SecretBox`](super::SecretBox)'s guarded
//! allocation, layered on top of what `memsec` already provides (private pages,
//! guard-page + canary, `mlock`, and `MADV_DONTDUMP` on Linux).
//!
//! Everything here is **best-effort**: a failed syscall weakens protection but
//! is never unsound, so results are ignored rather than propagated. All helpers
//! take `memsec`'s *user* pointer (the start of the value) and the value's byte
//! length.

// Platforms where we apply the fork/crash `madvise`/`minherit` advice. The
// range must be page-aligned, so these all go through `page_range`.
#[cfg(any(
    target_os = "linux",
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd",
    target_os = "dragonfly"
))]
fn page_range(ptr: *mut u8, len: usize) -> Option<(*mut core::ffi::c_void, usize)> {
    if len == 0 {
        return None;
    }
    // SAFETY: `sysconf` with a valid name has no preconditions.
    let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    if page <= 0 {
        return None;
    }
    let page = page as usize;
    let addr = ptr as usize;
    // The payload is right-aligned against the trailing guard page, so rounding
    // the start down to a page boundary stays inside `memsec`'s mlocked region.
    let start = addr & !(page - 1);
    let len = (addr + len - start + page - 1) & !(page - 1);
    Some((start as *mut core::ffi::c_void, len))
}

/// Exclude the secret's pages from crash dumps.
///
/// Windows: `WerRegisterExcludedMemoryBlock`. Elsewhere a no-op — `memsec`
/// already sets `MADV_DONTDUMP` on Linux, and macOS has no equivalent knob.
pub(super) fn exclude_from_dumps(ptr: *mut u8, len: usize) {
    #[cfg(windows)]
    // SAFETY: `ptr`/`len` describe a live, owned allocation; WER only records
    // the range, it does not dereference it.
    unsafe {
        let _ = windows_sys::Win32::System::ErrorReporting::WerRegisterExcludedMemoryBlock(
            ptr as *const core::ffi::c_void,
            len as u32,
        );
    }
    #[cfg(not(windows))]
    let _ = (ptr, len);
}

/// Advise the kernel to wipe the secret's pages in a forked child.
///
/// Linux: `madvise(MADV_WIPEONFORK)`. macOS/BSD: `minherit(VM_INHERIT_NONE)`
/// (the child does not inherit the region). Elsewhere a no-op. This can cause
/// surprising behaviour in `fork`ed children — see the [`SecretArc`] docs.
///
/// [`SecretArc`]: super::SecretArc
pub(super) fn advise_wipe_on_fork(ptr: *mut u8, len: usize) {
    #[cfg(target_os = "linux")]
    if let Some((addr, len)) = page_range(ptr, len) {
        // SAFETY: `addr`/`len` is a page-aligned sub-range of the live mapping.
        unsafe { libc::madvise(addr, len, libc::MADV_WIPEONFORK) };
    }
    #[cfg(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd",
        target_os = "dragonfly"
    ))]
    if let Some((addr, len)) = page_range(ptr, len) {
        // `minherit(2)` is a real libSystem/BSD function but has no `libc`
        // binding; declare it ourselves. `VM_INHERIT_NONE` (2) makes the region
        // absent — hence zero-filled — in a forked child.
        unsafe extern "C" {
            fn minherit(
                addr: *mut core::ffi::c_void,
                len: usize,
                inherit: core::ffi::c_int,
            ) -> core::ffi::c_int;
        }
        const VM_INHERIT_NONE: core::ffi::c_int = 2;
        // SAFETY: page-aligned sub-range of the live mapping.
        unsafe { minherit(addr, len, VM_INHERIT_NONE) };
    }
    #[cfg(not(any(
        target_os = "linux",
        target_os = "macos",
        target_os = "ios",
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd",
        target_os = "dragonfly"
    )))]
    let _ = (ptr, len);
}

/// Advise the kernel to zero the secret's wired pages on crash.
///
/// macOS/iOS: `madvise(MADV_ZERO_WIRED_PAGES)`. Elsewhere a no-op.
pub(super) fn advise_zero_wired_on_crash(ptr: *mut u8, len: usize) {
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    if let Some((addr, len)) = page_range(ptr, len) {
        // SAFETY: page-aligned sub-range of the live mapping.
        unsafe { libc::madvise(addr, len, libc::MADV_ZERO_WIRED_PAGES) };
    }
    #[cfg(not(any(target_os = "macos", target_os = "ios")))]
    let _ = (ptr, len);
}

/// Whether the secret is ciphered in memory while idle on this platform.
///
/// Only Windows (`CryptProtectMemory`), and only when the value length is a
/// non-zero multiple of the cipher block size — we cannot pad the encrypted
/// region past `memsec`'s canary or trailing guard page.
pub(super) fn encrypts<T>() -> bool {
    #[cfg(windows)]
    {
        const BLOCK: usize =
            windows_sys::Win32::Security::Cryptography::CRYPTPROTECTMEMORY_BLOCK_SIZE as usize;
        let n = size_of::<T>();
        n > 0 && n.is_multiple_of(BLOCK)
    }
    #[cfg(not(windows))]
    {
        let _ = size_of::<T>();
        false
    }
}

/// Encrypt the secret in place. Only meaningful (and only called) when
/// [`encrypts`] is true; the pages must be writable.
pub(super) fn encrypt(ptr: *mut u8, len: usize) {
    #[cfg(windows)]
    // SAFETY: caller guarantees `len` is a non-zero multiple of the block size
    // and the pages are writable.
    unsafe {
        let _ = windows_sys::Win32::Security::Cryptography::CryptProtectMemory(
            ptr as *mut core::ffi::c_void,
            len as u32,
            windows_sys::Win32::Security::Cryptography::CRYPTPROTECTMEMORY_SAME_PROCESS,
        );
    }
    #[cfg(not(windows))]
    let _ = (ptr, len);
}

/// Decrypt the secret in place. The inverse of [`encrypt`].
pub(super) fn decrypt(ptr: *mut u8, len: usize) {
    #[cfg(windows)]
    // SAFETY: as `encrypt`.
    unsafe {
        let _ = windows_sys::Win32::Security::Cryptography::CryptUnprotectMemory(
            ptr as *mut core::ffi::c_void,
            len as u32,
            windows_sys::Win32::Security::Cryptography::CRYPTPROTECTMEMORY_SAME_PROCESS,
        );
    }
    #[cfg(not(windows))]
    let _ = (ptr, len);
}
