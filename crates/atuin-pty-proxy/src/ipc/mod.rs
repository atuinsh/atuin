//! IPC operations on the `pty-proxy`.
mod client;
mod controller;
pub mod domain;
mod server;

pub use client::IpcClient;
pub use controller::IpcController;
pub(crate) use server::{IpcServer, IpcServerError, IpcSpawnError};
