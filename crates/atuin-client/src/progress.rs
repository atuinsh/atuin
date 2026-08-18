use std::{io::IsTerminal, sync::Arc};

use indicatif::ProgressDrawTarget;

/// Draw to stderr only when it is a terminal. `indicatif` does not check this itself, and
/// would otherwise write cursor escapes and a copy of the bar per redraw into pipes and logs.
pub fn draw_target() -> ProgressDrawTarget {
    if std::io::stderr().is_terminal() {
        ProgressDrawTarget::stderr()
    } else {
        ProgressDrawTarget::hidden()
    }
}

/// An optional callback, notified as an operation enters each of its stages. This keeps the
/// operations themselves free of any particular progress display.
pub struct Observer<T>(Option<Arc<dyn Fn(T) + Send + Sync>>);

impl<T> Observer<T> {
    pub const fn hidden() -> Self {
        Self(None)
    }

    pub fn new(f: impl Fn(T) + Send + Sync + 'static) -> Self {
        Self(Some(Arc::new(f)))
    }

    pub fn notify(&self, event: T) {
        if let Some(f) = &self.0 {
            f(event);
        }
    }
}

// Derived, this would demand `T: Clone`, but `T` flows through rather than being stored.
impl<T> Clone for Observer<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_non_terminal_stderr_gets_a_hidden_target() {
        assert!(!std::io::stderr().is_terminal(), "sanity: no tty in tests");
        assert!(draw_target().is_hidden());
    }
}
