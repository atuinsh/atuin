//! The agents setup knows: `atuin_common`'s, or none in a build without AI, so the screen and
//! its flags read the same either way.

#[cfg(feature = "ai")]
pub(super) use atuin_common::harnesstools::{AnyHarness, Harness};
#[cfg(not(feature = "ai"))]
pub(super) use none::{AnyHarness, Harness};

#[cfg(not(feature = "ai"))]
mod none {
    /// An agent, of which a build without AI knows none.
    #[derive(Clone, Copy, Debug)]
    pub(in super::super) enum AnyHarness {}

    impl AnyHarness {
        pub(in super::super) const fn all() -> &'static [Self] {
            &[]
        }

        pub(in super::super) fn from_name(name: &str) -> Result<Self, String> {
            Err(format!("can't set up `{name}`: this atuin was built without AI"))
        }
    }

    /// What setup reads of `atuin_common`'s `Harness`.
    pub(in super::super) trait Harness {
        fn name(&self) -> &'static str;
        fn display_name(&self) -> &'static str;
    }

    impl Harness for AnyHarness {
        fn name(&self) -> &'static str {
            match *self {}
        }

        fn display_name(&self) -> &'static str {
            match *self {}
        }
    }
}
