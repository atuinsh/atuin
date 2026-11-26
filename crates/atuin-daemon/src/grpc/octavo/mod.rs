//! gRPC service exposing Octavo thread sharing state from the upload queue.

pub mod pb;

#[cfg(feature = "octavo")]
mod service;

#[cfg(feature = "octavo")]
pub use service::Service;
