//! IPC operations on the `pty-proxy`.
pub mod domain;
mod wire;

#[cfg(feature = "client")]
mod client;
#[cfg(feature = "server")]
mod controller;
#[cfg(feature = "server")]
mod server;

#[cfg(feature = "client")]
pub use client::{IpcClient, IpcConnection, IpcError};
#[cfg(feature = "server")]
pub use controller::IpcController;
#[cfg(feature = "server")]
pub use server::IpcServer;
