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
    progenitor::generate_api!(
        spec = { path = "openapi.json", relative_to = OutDir },
        interface = Positional,
        inner_type = crate::HookState,
        // The record types carry invariants and the frozen field names of the PASETO implicit
        // assertion, so the client uses atuin-domain's rather than generating look-alikes.
        replace = {
            EncryptedData = ::atuin_domain::record::EncryptedData: ?FromStr + ?Display,
            Host = ::atuin_domain::record::Host: ?FromStr + ?Display,
            Record = ::atuin_domain::record::Record<::atuin_domain::record::EncryptedData>: ?FromStr + ?Display,
            RecordStatus = ::atuin_domain::record::RecordStatus: ?FromStr + ?Display,
        },
        convert = {
            { type = "string", format = "password" } = crate::Secret: ?FromStr + ?Display,
            { type = "integer", format = "int64", minimum = 0 } = u64: Default,
            { type = "string", format = "date-time" } = crate::DateTime: ?FromStr + ?Display,
            { type = "string", format = "uri" } = ::url::Url,
        },
        patch = {
            ModelInfo = { derives = [PartialEq, Eq] },
            ModelList = { derives = [PartialEq, Eq] },
            UsageBucket = { derives = [PartialEq, Eq] },
            UsageSnapshot = { derives = [PartialEq, Eq] },
        },
    );
}
mod header;
mod hooks;

pub use base_url::BaseUrlError;
pub use caps::{AuthHeaderFuture, AuthHeaderProvider, CapClient, CapMismatch, ServerSupportError};
pub use date_time::DateTime;
pub use error::{ApiBody, ApiError, MapApiError};
pub use generated::{Client, ResponseValue, types};
pub use header::{AuthToken, ClientBuildError};
pub use hooks::HookState;

/// A secret the API carries in plaintext; its `Debug` stays redacted.
#[derive(Clone, Debug, Deserialize, Serialize, From, Into, Deref)]
#[from(SecretString, String, &str)]
#[serde(transparent)]
pub struct Secret(#[serde(serialize_with = "expose")] SecretString);

fn expose<S: Serializer>(secret: &SecretString, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(secret.expose_secret())
}
