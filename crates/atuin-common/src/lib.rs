#![deny(unsafe_code)]
#![cfg_attr(test, allow(clippy::disallowed_methods, reason = "tests may use std::fs for fixtures"))]
// TODO(markovejnovic): remove once atuin-common's non-wrapper modules are migrated (fd-pool
// migration)
#![cfg_attr(not(test), allow(clippy::disallowed_methods))]

#[cfg(feature = "ansi")]
pub mod ansi;
#[cfg(feature = "db")]
pub mod db;
pub mod docs;
pub mod encryption;
pub mod filter;
pub mod fs;
pub mod futures;
#[cfg(feature = "ai")]
pub mod harnesstools;
pub mod logs;
pub mod os;
pub mod path;
pub mod range;
pub mod rmp;
pub mod secrets;
pub mod shell;
pub mod slice;
pub mod string;
pub mod sync;
#[cfg(feature = "test-utils")]
pub mod test_utils;
pub mod time;
pub mod units;
pub mod url;
pub mod utils;
