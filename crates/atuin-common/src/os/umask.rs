//! Utilities for managing the process's umask.

use std::sync::OnceLock;

/// The process's original umask before [`Umask::install`] was run.
static ORIGINAL: OnceLock<Umask> = OnceLock::new();

#[derive(Clone, Copy, Debug, PartialEq, Eq, derive_more::Display)]
#[display("{_0:04o}")]
pub struct Umask(u32);

impl Umask {
    /// `077` umask: created files are accessible only by the owner.
    pub const OWNER_ONLY: Self = Self(0o077);

    #[must_use]
    pub const fn from_bits(bits: u32) -> Self {
        Self(bits & 0o777)
    }

    #[must_use]
    pub const fn bits(self) -> u32 {
        self.0
    }

    /// Set this process's umask to `self`. Returns the old umask.
    ///
    /// The first time this method is called, the process's original umask will be remembered in
    /// [`Umask::original`]. Future calls to this method won't overwrite that value.
    #[cfg(unix)]
    #[allow(
        clippy::must_use_candidate,
        clippy::return_self_not_must_use,
        reason = "setting the umask is the point; the previous one is often not needed"
    )]
    pub fn install(self) -> Self {
        let previous = self.install_raw();
        let _ = ORIGINAL.set(previous);
        previous
    }

    /// Async-signal-safe version of [`Self::install`] that doesn't touch [`Umask::original`].
    ///
    /// In general, prefer [`Self::install`]. This method is for specific cases like `pre_exec`
    /// hooks that require async-signal-safety.
    #[cfg(unix)]
    #[allow(
        clippy::useless_conversion,
        clippy::cast_possible_truncation,
        reason = "mode_t is u16 on macOS and u32 on Linux"
    )]
    #[allow(
        clippy::must_use_candidate,
        clippy::return_self_not_must_use,
        reason = "setting the umask is the point; the previous one is often not needed"
    )]
    pub fn install_raw(self) -> Self {
        let mode = rustix::fs::Mode::from_bits_truncate(self.0 as _);
        Self::from_bits(u32::from(rustix::process::umask(mode).bits()))
    }

    /// The original umask this process had, before [`Self::install`] was ever called.
    ///
    /// [`None`] if [`Self::install`] was never called.
    #[must_use]
    pub fn original() -> Option<Self> {
        ORIGINAL.get().copied()
    }
}

#[cfg(all(test, unix))]
mod tests {
    use rstest::rstest;

    use super::*;

    #[rstest]
    fn only_install_remembers_the_first_umask_it_replaced() {
        // One test, not two: `ORIGINAL` and the umask are process-wide.
        let launched = Umask::OWNER_ONLY.install_raw();
        assert_eq!(Umask::original(), None);
        assert_eq!(launched.install_raw(), Umask::OWNER_ONLY);

        let launched = Umask::OWNER_ONLY.install();
        // A second call must not overwrite the original with our own `077`.
        assert_eq!(Umask::from_bits(0o022).install(), Umask::OWNER_ONLY);

        assert_eq!(Umask::original(), Some(launched));

        // Put back the test process's umask.
        assert_eq!(launched.install(), Umask::from_bits(0o022));
    }

    #[rstest]
    #[case(0o022, "0022")]
    #[case(0o077, "0077")]
    #[case(0o7777, "0777")]
    fn displays_as_octal(#[case] bits: u32, #[case] expected: &str) {
        assert_eq!(Umask::from_bits(bits).to_string(), expected);
    }
}
