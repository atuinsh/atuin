#![deny(unsafe_code)]
#![warn(clippy::pedantic, clippy::nursery)]

//! Typed client for the Atuin sync, hub and AI HTTP APIs.
//!
//! The hub owns the contract: `mix api.spec` writes it to this crate's `openapi.json`, and
//! `codegen/` turns that into `src/generated.rs` with progenitor. Everything else here is the
//! hand-written runtime the generated code calls into: [`Secret`] and [`DateTime`] for the mapped
//! schema formats, the hooks behind [`Client::with_capabilities`] and [`Client::with_auth`], the
//! [`CapClient`] they negotiate with, the base URL handling of [`Client::from_http`],
//! [`authorization`] for the token header callers configure, and [`ApiError`], which every failed
//! call maps into.
//!
//! One [`Client`] serves one base URL with one HTTP policy: build it from a configured
//! [`reqwest::Client`] with [`Client::from_http`], never with the generated `Client::new`, whose
//! 15 s timeouts are not Atuin's, or `Client::new_with_client`, which skips the base URL handling.
//! clippy.toml bans both.
//!
//! ```no_run
//! # async fn run(base: url::Url, http: reqwest::Client) -> Result<(), Box<dyn std::error::Error>> {
//! use atuin_api_client::{Client, MapApiError};
//!
//! let client = Client::from_http(&base, http)?;
//! let me = client.get_me().map_api_error().await?;
//! println!("{:?} (server {:?})", me.username, me.headers().get("atuin-version"));
//! # Ok(())
//! # }
//! ```

mod base_url;
mod caps;
mod date_time;
mod error;
#[rustfmt::skip]
#[allow(
    clippy::disallowed_methods,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::must_use_candidate,
    clippy::struct_field_names,
    clippy::unnecessary_trailing_comma,
    reason = "generated from openapi.json by progenitor"
)]
mod generated;
mod header;
mod hooks;
mod secret;

pub use base_url::BaseUrlError;
pub use caps::{AuthHeaderFuture, AuthHeaderProvider, CapClient, CapMismatch, ServerSupportError};
pub use date_time::DateTime;
pub use error::{ApiError, MapApiError};
pub use generated::{Client, ResponseValue, types};
pub use header::authorization;
pub use hooks::HookState;
pub use secret::Secret;
