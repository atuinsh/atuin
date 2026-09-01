//! The pty-proxy is a service which runs in the user's shell, intercepts terminal data and then
//! provides structured views into terminal data.
//!
//! The crate is organized into two modules:
//!
//!   - [`self::server`] is an embeddable service which you can add into any process to spawn the
//!     proxy runtime. This server exposes an IPC server which you can use to query the pty-proxy
//!     from other processes.
//!   - [`self::client`] is a client you can use to communicate with the `pty-proxy`. This client
//!     talks to a running [`self::server`] in runtime.
#[cfg(all(unix, feature = "domain"))]
mod domain;

#[cfg(feature = "server")]
mod server;

#[cfg(all(unix, feature = "client"))]
mod client;

#[cfg(all(unix, feature = "client"))]
pub use client::{IpcClient, IpcConnectError, IpcConnection, IpcError};
#[cfg(all(unix, feature = "domain"))]
pub use domain::ScreenSnapshot;
#[cfg(feature = "server")]
pub use server::cli;
#[cfg(all(unix, feature = "server"))]
pub use server::{CommandCapture, CommandCaptureSink, cli::Shell, init_script};
