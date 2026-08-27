//! A wrapper around [`rmp`], with additional utilities and better error types.

pub use rmp::Marker;

pub mod decode;
pub mod encode;
