//! [`rmp`]-related utilities.
//!
//! Small helpers that fill gaps in the upstream `rmp` and `rmp_serde` crates.

pub use rmp::Marker;

pub mod decode;
pub mod encode;
pub mod serde;
