//! IPC operations on the `pty-proxy`.
mod client;
mod controller;
pub mod domain;
mod server;
mod wire;

pub use client::{IpcClient, IpcConnection, IpcError};
pub use controller::IpcController;
pub use server::IpcServer;
