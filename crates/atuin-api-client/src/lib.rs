//! Typed client for the Atuin sync, hub and AI HTTP APIs.

use derive_more::{Deref, From, Into};
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize, Serializer};
use url::Url;

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

pub use date_time::DateTime;
pub use error::{ApiBody, ApiError, MapApiError};
pub use generated::{Client, ResponseValue, types};
pub use header::{AuthToken, ClientBuildError};
pub use hooks::{AuthHeaderFuture, AuthHeaderProvider, HookState};

/// A secret the API carries in plaintext; its `Debug` stays redacted.
#[derive(Clone, Debug, Deserialize, Serialize, From, Into, Deref)]
#[from(SecretString, String, &str)]
#[serde(transparent)]
pub struct Secret(#[serde(serialize_with = "expose")] SecretString);

fn expose<S: Serializer>(secret: &SecretString, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(secret.expose_secret())
}

impl Client {
    /// Build a client for the server at `base` that sends every request through `http`.
    pub fn from_http(mut base: Url, http: reqwest::Client) -> Result<Self, ClientBuildError> {
        if base.cannot_be_a_base() {
            return Err(ClientBuildError::CannotBeABase);
        }
        // The generated code formats `{prefix}/api/v0/me` as a string, so a trailing slash would
        // double and a query would land before the path; the query goes back on in the pre hook.
        let query = base.query().filter(|query| !query.is_empty()).map(str::to_owned);
        base.set_query(None);
        base.set_fragment(None);

        let prefix = base.as_str().trim_end_matches('/');

        #[allow(clippy::disallowed_methods, reason = "the one place that normalises the base")]
        Ok(Self::new_with_client(prefix, http, HookState::for_base(query)))
    }
}
