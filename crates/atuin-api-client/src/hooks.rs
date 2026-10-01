//! The hooks progenitor runs around every generated operation.
//!
//! progenitor stamps `api-version: <info.version>` on every request. No Atuin server reads it and
//! the hand-written clients never sent it, so `pre` removes it. `pre` also puts the base URL's
//! query ahead of the operation's, and resolves the [`AuthHeaderProvider`] of a
//! [`Client::with_auth`] client, per request.
//!
//! Capability negotiation stamps the token the [`CapClient`] last fetched as
//! `x-atuin-capabilities-known` (plus `x-atuin-capabilities-enforce` in [`CapMismatch::Error`]
//! mode), sends, and in [`CapMismatch::Continue`] mode refreshes in the background when the
//! answer's `x-atuin-capabilities-available` differs from both the token sent and the one now
//! cached. It overrides `exec` rather than splitting across `pre` and `post` because that
//! comparison needs the token the request carried, which `post` never sees. `get_capabilities` is
//! never negotiated: neither server negotiates that route, and a refresh must not trigger a
//! refresh.

use std::sync::Arc;

use atuin_domain::caps::http::{AVAILABLE_HEADER, ENFORCE_HEADER, KNOWN_HEADER};
use progenitor_client::{ClientHooks, Error, OperationInfo};
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderName, HeaderValue};

use crate::{AuthHeaderProvider, CapClient, CapMismatch, Client};

const API_VERSION_HEADER: &str = "api-version";
/// `OperationInfo::operation_id` of [`Client::get_capabilities`], which progenitor snake-cases.
const GET_CAPABILITIES: &str = "get_capabilities";

/// State the generated operations hand to the hooks: the base URL's query from
/// [`Client::from_http`], and what [`Client::with_capabilities`] and [`Client::with_auth`] set.
#[derive(Debug, Clone, Default)]
pub struct HookState {
    /// The query of the base URL, which progenitor's string-formatted paths cannot carry.
    base_query: Option<String>,
    negotiation: Option<Negotiation>,
    auth: Option<AuthHeaderProvider>,
}

impl HookState {
    pub(crate) fn for_base(base_query: Option<String>) -> Self {
        Self {
            base_query,
            ..Self::default()
        }
    }
}

#[derive(Debug, Clone)]
struct Negotiation {
    caps: Arc<CapClient>,
    on_mismatch: CapMismatch,
}

impl Client {
    /// Negotiate capabilities through `caps` on every operation but [`Client::get_capabilities`].
    #[must_use]
    pub fn with_capabilities(mut self, caps: Arc<CapClient>, on_mismatch: CapMismatch) -> Self {
        self.inner.negotiation = Some(Negotiation { caps, on_mismatch });
        self
    }

    /// Resolve `Authorization` through `auth` before every request, over any default header.
    #[must_use]
    pub fn with_auth(mut self, auth: AuthHeaderProvider) -> Self {
        self.inner.auth = Some(auth);
        self
    }
}

impl ClientHooks<HookState> for Client {
    async fn pre<E>(
        &self,
        request: &mut reqwest::Request,
        _info: &OperationInfo,
    ) -> Result<(), Error<E>> {
        request.headers_mut().remove(API_VERSION_HEADER);
        if let Some(base_query) = &self.inner.base_query {
            let url = request.url_mut();
            let query = url
                .query()
                .map_or_else(|| base_query.clone(), |query| format!("{base_query}&{query}"));
            url.set_query(Some(&query));
        }
        if let Some(auth) = &self.inner.auth
            && let Some(value) = auth.resolve().await
        {
            request.headers_mut().insert(AUTHORIZATION, value);
        }
        Ok(())
    }

    async fn exec(
        &self,
        request: reqwest::Request,
        info: &OperationInfo,
    ) -> reqwest::Result<reqwest::Response> {
        match &self.inner.negotiation {
            Some(negotiation) if info.operation_id != GET_CAPABILITIES => {
                negotiation.exec(&self.client, request).await
            }
            Some(_) | None => self.client.execute(request).await,
        }
    }
}

impl Negotiation {
    async fn exec(
        &self,
        http: &reqwest::Client,
        mut request: reqwest::Request,
    ) -> reqwest::Result<reqwest::Response> {
        // A `None` or non-ASCII token leaves the header off rather than failing the request.
        let known = self.caps.known_token();
        if let Some(value) = known.as_deref().and_then(|token| HeaderValue::from_str(token).ok()) {
            request.headers_mut().insert(HeaderName::from_static(KNOWN_HEADER), value);
        }
        match self.on_mismatch {
            CapMismatch::Error => {
                request
                    .headers_mut()
                    .insert(HeaderName::from_static(ENFORCE_HEADER), HeaderValue::from_static("1"));
            }
            CapMismatch::Continue => {}
        }

        let response = http.execute(request).await?;
        self.refresh_if_advertised(response.headers(), known.as_deref());
        Ok(response)
    }

    /// Refresh in the background if `headers` advertise the token [`token_to_refresh`] picks.
    ///
    /// Not inlined into `exec`: the refresh runs `get_capabilities` through `exec` again, and
    /// rustc proves that future `Send` only from outside `exec`'s own body.
    fn refresh_if_advertised(&self, headers: &HeaderMap, sent: Option<&str>) {
        let available = headers.get(AVAILABLE_HEADER).and_then(|value| value.to_str().ok());
        let cached = self.caps.known_token();
        let Some(available) =
            token_to_refresh(self.on_mismatch, available, sent, cached.as_deref())
        else {
            return;
        };
        let caps = Arc::clone(&self.caps);
        let available = available.to_owned();
        // Detached on purpose: the request is already served and nothing awaits the refresh.
        // `refresh_if_stale` coalesces, so a burst drives one fetch, and a failure must not fail
        // the request, so it is dropped.
        tokio::spawn(async move {
            let _ = caps.refresh_if_stale(&available).await;
        });
    }
}

/// The capability token to refresh to after an answer advertised `available`, if any.
///
/// Only [`CapMismatch::Continue`] refreshes, and only to a token that differs from both `sent`,
/// the one the request carried, and `cached`, the one cached now.
fn token_to_refresh<'a>(
    on_mismatch: CapMismatch,
    available: Option<&'a str>,
    sent: Option<&str>,
    cached: Option<&str>,
) -> Option<&'a str> {
    match on_mismatch {
        CapMismatch::Error => None,
        CapMismatch::Continue => {
            available.filter(|available| Some(*available) != sent && Some(*available) != cached)
        }
    }
}
