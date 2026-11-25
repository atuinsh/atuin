//! Utilities for caught panics.

use std::any::Any;

/// What a panic said, when it said it as text.
#[must_use]
pub fn message(panic: &(dyn Any + Send)) -> &str {
    panic
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| panic.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("(not text)")
}

#[cfg(test)]
mod tests {
    use std::panic::catch_unwind;

    use rstest::rstest;

    use super::*;

    #[rstest]
    fn reads_the_text_of_a_panic() {
        let literal = catch_unwind(|| panic!("literal")).expect_err("a panic");
        let formatted = catch_unwind(|| panic!("formatted {}", 1)).expect_err("a panic");
        let other = catch_unwind(|| std::panic::panic_any(7_u8)).expect_err("a panic");

        assert_eq!(message(&*literal), "literal");
        assert_eq!(message(&*formatted), "formatted 1");
        assert_eq!(message(&*other), "(not text)");
    }
}
