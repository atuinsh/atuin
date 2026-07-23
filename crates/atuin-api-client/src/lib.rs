//! Client for the atuin sync server HTTP API.
//!
//! The contents of this crate are generated at build time by `build.rs` from
//! `crates/atuin-client/openapi.json`, which is in turn generated from
//! `atuin-server`. See README.md; nothing here is written by hand.

include!(concat!(env!("OUT_DIR"), "/codegen.rs"));
