//! A wrapper around [`rmp`], with additional utilities and better error types.

pub use rmp::Marker;

pub mod byte_buf;
pub mod decode;
pub mod encode;
#[cfg(feature = "rmp-frame")]
pub mod frame;
