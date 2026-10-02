//! macOS-specific utilities.
//!
//! Note: this module is also compiled on `cfg(all(test, unix))`, so that some code can be tested on
//! non-macOS systems. This is the case for the [`responsibility`] module (currently the only
//! module); if you add a module that you wish to exclude from this behavior, annotate it with
//! `#[cfg(target_os = "macos")]`.

mod responsibility;

pub use responsibility::{SpawnDisclaimedError, spawn_disclaimed};
