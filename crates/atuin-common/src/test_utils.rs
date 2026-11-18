//! Utilities for use in tests.
pub mod capture;
#[cfg(unix)]
pub mod read_only;
pub use capture::capture_logs;
#[cfg(unix)]
pub use read_only::ReadOnlyTree;
