//! Typed client for the Atuin sync, hub and AI HTTP APIs.

use derive_more::{Deref, From, Into};
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize, Serializer};

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

pub use base_url::BaseUrlError;
pub use caps::{AuthHeaderFuture, AuthHeaderProvider, CapClient, CapMismatch, ServerSupportError};
pub use date_time::DateTime;
pub use error::{ApiError, MapApiError};
pub use generated::{Client, ResponseValue, types};
pub use header::authorization;
pub use hooks::HookState;

/// A secret the API carries in plaintext; its `Debug` stays redacted.
#[derive(Clone, Debug, Deserialize, Serialize, From, Into, Deref)]
#[from(SecretString, String, &str)]
#[serde(transparent)]
pub struct Secret(#[serde(serialize_with = "expose")] SecretString);

fn expose<S: Serializer>(secret: &SecretString, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(secret.expose_secret())
}
