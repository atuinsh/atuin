//! Macro to check if the radix is valid, in generic code.

// RADIX

/// Check radix is in range [2, 36] in debug builds.
#[cfg(feature = "radix")]
macro_rules! debug_assert_radix {
    ($radix:expr) => (debug_assert!($radix.as_i32() >= 2 && $radix.as_i32() <= 36, "Numerical base must be from 2-36.");)
}

/// Check radix is equal to 10.
#[cfg(not(feature = "radix"))]
macro_rules! debug_assert_radix {
    ($radix:expr) => (debug_assert!($radix.as_i32() == 10, "Numerical base must be 10.");)
}

/// Check radix is in range [2, 36] in debug and release builds.
#[cfg(feature = "radix")]
macro_rules! assert_radix {
    ($radix:expr) => (assert!($radix.as_i32() >= 2 && $radix.as_i32() <= 36, "Numerical base must be from 2-36.");)
}

// BUFFER

/// Check the buffer has sufficient room for the output.
macro_rules! assert_buffer {
    ($radix:expr, $slc:ident, $t:ty) => ({
        #[cfg(feature = "radix")]
        match $radix {
            10 => assert!($slc.len() >= <$t>::FORMATTED_SIZE_DECIMAL),
            _  => assert!($slc.len() >= <$t>::FORMATTED_SIZE),
        }

        #[cfg(not(feature = "radix"))]
        assert!($slc.len() >= <$t>::FORMATTED_SIZE);
    });
}
