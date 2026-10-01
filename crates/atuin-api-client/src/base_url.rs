use url::Url;

use crate::{Client, HookState};

/// Why a URL cannot be a [`Client`]'s base.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum BaseUrlError {
    #[error("a base URL needs a hierarchical path, like https://host/prefix")]
    CannotBeABase,
}

impl Client {
    /// Build a client for the server at `base` that sends every request through `http`.
    ///
    /// `base` keeps its path prefix and drops trailing slashes, as `UrlAppendExt::append_path`
    /// does, so a self-hosted server can live under `https://host/atuin/`. Its query goes ahead of
    /// every operation's own, and its fragment is never sent.
    ///
    /// # Errors
    ///
    /// [`BaseUrlError`] when paths cannot follow `base`.
    pub fn from_http(base: &Url, http: reqwest::Client) -> Result<Self, BaseUrlError> {
        let Base { prefix, query } = Base::parse(base)?;
        #[allow(clippy::disallowed_methods, reason = "the one place that normalises the base")]
        Ok(Self::new_with_client(&prefix, http, HookState::for_base(query)))
    }
}

/// A base URL split the way progenitor needs it.
struct Base {
    /// The prefix the generated code formats operation paths onto, e.g. `https://host/atuin`.
    ///
    /// It formats `{prefix}/api/v0/me` as a string, so a trailing slash would double and a query
    /// would land before the path.
    prefix: String,
    /// The base's query, e.g. `tok=1`, if it has a non-empty one.
    query: Option<String>,
}

impl Base {
    fn parse(base: &Url) -> Result<Self, BaseUrlError> {
        if base.cannot_be_a_base() {
            return Err(BaseUrlError::CannotBeABase);
        }
        let mut prefix = base.clone();
        prefix.set_query(None);
        prefix.set_fragment(None);
        Ok(Self {
            prefix: prefix.as_str().trim_end_matches('/').to_owned(),
            query: base.query().filter(|query| !query.is_empty()).map(str::to_owned),
        })
    }
}
