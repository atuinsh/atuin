//! Kiro CLI (`kiro-cli`): installing its hooks.
//!
//! Only the hook installer lives here for now; the `Kiro` harness (sessions, resume, locate)
//! lands with the reader and wires [`hooks::install`] into its `Harness` impl.

#[allow(dead_code)]
pub mod hooks;
