//! Macro for checking CPU capabilities at runtime.
//!
//! # Usage example
//! ```
//! if cpuid_bool::cpuid_bool!("sha", "aes") {
//!     println!("CPU supports both SHA and AES extensions");
//! } else {
//!     println!("SHA and AES extensions are not supported");
//! }
//! ```
//! Note that if all tested target features are enabled via compiler options
//! (e.g. by using `RUSTFLAGS`), `cpuid_bool!` macro immideatly will expand
//! to `true` and will not use CPUID instruction. Such behavior allows
//! compiler to eliminate fallback code.
//!
//! After first call macro caches result and returns it in subsequent
//! calls, thus runtime overhead for them is minimal.
#![no_std]
#[cfg(not(any(target_arch = "x86", target_arch = "x86_64")))]
compile_error!("This crate works only on x86 and x86-64 targets.");

use core::sync::atomic::{AtomicU8, Ordering::Relaxed};

/// This structure represents a lazily initialized static boolean value.
///
/// Useful when it is preferable to just rerun initialization instead of
/// locking. Used internally by the `cpuid_bool` macro.
pub struct LazyBool(AtomicU8);

impl LazyBool {
    const UNINIT: u8 = u8::max_value();

    pub const fn new() -> Self {
        Self(AtomicU8::new(Self::UNINIT))
    }

    // Runs the init() function at least once, returning the value of some run
    // of init(). Multiple callers can run their init() functions in parallel.
    // init() should always return the same value, if it succeeds.
    pub fn unsync_init(&self, init: impl FnOnce() -> bool) -> bool {
        // Relaxed ordering is fine, as we only have a single atomic variable.
        let mut val = self.0.load(Relaxed);
        if val == Self::UNINIT {
            val = init() as u8;
            self.0.store(val as u8, Relaxed);
        }
        val != 0
    }
}

// TODO: find how to define private macro usable inside a public one
macro_rules! expand_check_macro {
    ($(($name:tt, $i:expr, $reg:ident, $offset:expr)),* $(,)?) => {
        #[macro_export]
        #[doc(hidden)]
        macro_rules! check {
            $(
                ($cr:expr, $name) => { ($cr[$i].$reg & (1 << $offset) != 0) };
            )*
        }
    };
}

expand_check_macro! {
    ("mmx", 0, edx, 23),
    ("sse", 0, edx, 25),
    ("sse2", 0, edx, 26),
    ("sse3", 0, ecx, 0),
    ("pclmulqdq", 0, ecx, 1),
    ("ssse3", 0, ecx, 9),
    ("fma", 0, ecx, 12),
    ("sse4.1", 0, ecx, 19),
    ("sse4.2", 0, ecx, 20),
    ("popcnt", 0, ecx, 23),
    ("aes", 0, ecx, 25),
    ("avx", 0, ecx, 28),
    ("rdrand", 0, ecx, 30),
    ("sgx", 1, ebx, 2),
    ("bmi1", 1, ebx, 3),
    ("avx2", 1, ebx, 5),
    ("bmi2", 1, ebx, 8),
    ("rdseed", 1, ebx, 18),
    ("adx", 1, ebx, 19),
    ("sha", 1, ebx, 29),
}

/// Check at runtime if CPU supports sequence of target features.
///
/// During first execution this macro will use CPUID to check requested
/// target features, results will be cached and further calls will return
/// it instead.
#[macro_export]
macro_rules! cpuid_bool {
    ($($tf:tt),+ $(,)? ) => {{
        // CPUID is not available on SGX targets
        #[cfg(all(not(target_env = "sgx"), not(all($(target_feature=$tf, )*))))]
        let res = {
            #[cfg(target_arch = "x86")]
            use core::arch::x86::{__cpuid, __cpuid_count};
            #[cfg(target_arch = "x86_64")]
            use core::arch::x86_64::{__cpuid, __cpuid_count};

            static CPUID_BOOL: cpuid_bool::LazyBool = cpuid_bool::LazyBool::new();
            CPUID_BOOL.unsync_init(|| {
                #[allow(unused_variables)]
                let cr = unsafe {
                    [__cpuid(1), __cpuid_count(7, 0)]
                };
                // TODO: find how to remove `true`
                $(cpuid_bool::check!(cr, $tf) & )+ true
            })
        };

        #[cfg(all(target_env = "sgx", not(all($(target_feature=$tf, )*))))]
        let res = false;
        #[cfg(all($(target_feature=$tf, )*))]
        let res = true;

        res
    }};
}
