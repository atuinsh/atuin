//! Macros to simple performance profiling using perftools.

/// Only inline when not profiling.
macro_rules! perftools_inline {
    ($($item:tt)*) => (
        #[cfg_attr(feature = "noinline", inline(never))]
        #[cfg_attr(not(feature = "noinline"), inline)]
        $($item)*
    );
}

/// Only inline when not profiling.
macro_rules! perftools_inline_always {
    ($($item:tt)*) => (
        #[cfg_attr(feature = "noinline", inline(never))]
        #[cfg_attr(not(feature = "noinline"), inline(always))]
        $($item)*
    )
}
