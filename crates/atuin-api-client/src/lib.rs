//! Typed client for the Atuin sync, hub and AI HTTP APIs.

mod base_url;
mod caps;
mod date_time;
mod error;
mod generated {
    #![allow(
        clippy::disallowed_methods,
        clippy::missing_errors_doc,
        clippy::missing_panics_doc,
        clippy::must_use_candidate,
        clippy::struct_field_names,
        clippy::unnecessary_trailing_comma,
        reason = "generated from openapi.json by progenitor"
    )]
    include!(concat!(env!("OUT_DIR"), "/generated.rs"));
}
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
