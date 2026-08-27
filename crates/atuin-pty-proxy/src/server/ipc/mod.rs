//! IPC server for the `pty-proxy`.
mod controller;
mod server;

pub use controller::IpcController;
pub use server::IpcServer;
