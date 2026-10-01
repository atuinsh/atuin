//! The client side of capability negotiation: the server's capabilities, fetched and cached.
//!
//! The protocol and the capability types live in `atuin_domain::caps`. [`CapClient`] fetches the
//! server's document with [`Client::get_capabilities`]; [`Client::with_capabilities`] stamps the
//! token it last fetched onto every other operation and refreshes it when the server advertises
//! another.

use std::fmt;
use std::pin::Pin;
use std::sync::Arc;

use atuin_domain::caps::Capability;
use parking_lot::RwLock;
use reqwest::header::HeaderValue;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};
use tokio::sync::{Mutex, watch};

use crate::{ApiError, Client, MapApiError, types};

/// The future an [`AuthHeaderProvider`] resolves an `Authorization` header with.
///
/// Mark the value [sensitive](HeaderValue::set_sensitive) so it stays out of `Debug` output.
pub type AuthHeaderFuture = Pin<Box<dyn Future<Output = Option<HeaderValue>> + Send>>;

/// Resolves the `Authorization` header for each request of a [`Client::with_auth`] client.
///
/// Returning `None` sends the request anonymously. Auth is resolved per request, not at
/// construction, so a long-lived client (e.g. the daemon's capability reader) follows login,
/// logout, and token rotation without a rebuild.
#[derive(Clone)]
pub struct AuthHeaderProvider(Arc<dyn Fn() -> AuthHeaderFuture + Send + Sync>);

impl AuthHeaderProvider {
    pub fn new(f: impl Fn() -> AuthHeaderFuture + Send + Sync + 'static) -> Self {
        Self(Arc::new(f))
    }

    pub(crate) async fn resolve(&self) -> Option<HeaderValue> {
        (self.0)().await
    }
}

impl fmt::Debug for AuthHeaderProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("AuthHeaderProvider")
    }
}

/// How a [`Client::with_capabilities`] client reacts when its capability token is out of date with
/// the server's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapMismatch {
    /// Let the server serve the request despite the mismatch, then refresh capabilities in the
    /// background so later requests are current. The original request is never resent.
    Continue,
    /// Ask the server (via `x-atuin-capabilities-enforce`) to reject the request with `412` on a
    /// mismatch, surfacing it to the caller.
    Error,
}

/// The server's capabilities, as the client last fetched them.
///
/// They are populated by [`CapClient::refresh`], which runs once on construction and again
/// whenever negotiation finds the cache stale; [`CapClient::get_server`] is then a read of that
/// cache.
///
/// Thread it as an [`Arc`].
#[derive(Debug)]
pub struct CapClient {
    /// The server's capabilities as last fetched; `None` until the first refresh. Cheap concurrent
    /// reads; writes are serialized by `fetching`.
    server: RwLock<Option<ServerCaps>>,
    /// Serializes capability fetches so a burst of stale callers makes a single network hop.
    fetching: Mutex<()>,
    /// Fetches the server's document. Never negotiates: `get_capabilities` is exempt.
    api: Client,
    /// Flips to `true` once the warm-up fetch has finished, whether or not it succeeded.
    warmed: watch::Receiver<bool>,
}

/// The capabilities a server advertises, as last fetched from its capabilities endpoint.
#[derive(Debug)]
struct ServerCaps {
    /// The server's capability token.
    version: String,
    caps: Map<String, Value>,
}

impl From<types::CapabilitiesResponse> for ServerCaps {
    fn from(document: types::CapabilitiesResponse) -> Self {
        Self {
            version: document.version,
            caps: document.capabilities,
        }
    }
}

/// Why reading a server capability could not yield a value.
#[derive(Debug, thiserror::Error)]
pub enum ServerSupportError {
    /// Capabilities have not been fetched from the server yet -- the caller may want to
    /// [`CapClient::refresh`] and ask again. This is an absence of knowledge, distinct from the
    /// server telling us it does not advertise the capability (which is `Ok(None)`).
    #[error("server capabilities have not been fetched yet")]
    NotFetched,
    /// The server advertises the capability, but its value did not deserialize into the type the
    /// caller asked for -- typically a version skew, or the wrong type for the name.
    #[error("server capability {name:?} did not deserialize into the requested type")]
    Malformed {
        name: &'static str,
        #[source]
        source: serde_json::Error,
    },
}

impl CapClient {
    /// Read the capabilities of the server `api` talks to, starting a warm-up fetch now.
    ///
    /// Must run inside a tokio runtime. Give `api` [`Client::with_auth`] to fetch the document the
    /// server scopes to the current user.
    #[must_use]
    pub fn new(api: Client) -> Arc<Self> {
        let (warmed, warmed_rx) = watch::channel(false);
        let new = Arc::new(Self {
            server: RwLock::new(None),
            fetching: Mutex::new(()),
            api,
            warmed: warmed_rx,
        });

        let this = Arc::clone(&new);
        // Detached on purpose: only `get_server` waits on it, through `warmed`, and a failed fetch
        // leaves the cache empty, which `get_server` reports as `NotFetched`.
        tokio::spawn(async move {
            let _ = this.refresh().await;
            let _ = warmed.send(true);
        });

        new
    }

    /// Fetch the server's capabilities and replace the cache with them.
    ///
    /// Fetches are serialized so parallel callers never overlap; this one always fetches.
    ///
    /// # Errors
    ///
    /// [`ApiError`] when the fetch fails, leaving the cache as it was.
    pub async fn refresh(&self) -> Result<(), ApiError> {
        let _fetching = self.fetching.lock().await;
        let caps = self.fetch_server_caps().await?;
        *self.server.write() = Some(caps);
        Ok(())
    }

    /// Refresh the server's capabilities only if the cached token differs from `available`.
    ///
    /// Double-checked so a burst of stale callers makes a single fetch, and a caller whose token
    /// already matches `available` does no work.
    ///
    /// # Errors
    ///
    /// [`ApiError`] when the fetch fails, leaving the cache as it was.
    pub async fn refresh_if_stale(&self, available: &str) -> Result<(), ApiError> {
        if !self.is_stale(available) {
            return Ok(());
        }
        let _fetching = self.fetching.lock().await;
        if !self.is_stale(available) {
            return Ok(());
        }
        let caps = self.fetch_server_caps().await?;
        *self.server.write() = Some(caps);
        Ok(())
    }

    /// Whether the cached server token differs from `available` (or nothing is cached yet).
    fn is_stale(&self, available: &str) -> bool {
        self.server.read().as_ref().map(|caps| caps.version.as_str()) != Some(available)
    }

    async fn fetch_server_caps(&self) -> Result<ServerCaps, ApiError> {
        let document = self.api.get_capabilities().map_api_error().await?;
        Ok(document.into_inner().into())
    }

    /// Read whether the server supports the given capability, from the last [`CapClient::refresh`].
    ///
    /// Waits for the warm-up fetch first.
    ///
    /// - `Ok(Some(c))` - the server advertises the capability and it deserialized into `C`.
    /// - `Ok(None)` - fetched, and the server does not advertise it (a definitive "no").
    /// - `Err(ServerSupportError::NotFetched)` - capabilities have not been fetched yet.
    /// - `Err(ServerSupportError::Malformed)` - advertised, but its value did not deserialize into
    ///   `C`. The caller decides whether that is fatal or a reason to fall back.
    ///
    /// # Errors
    ///
    /// [`ServerSupportError`], as above.
    pub async fn get_server<C: Capability + DeserializeOwned>(
        &self,
    ) -> Result<Option<C>, ServerSupportError> {
        let _ = self.warmed.clone().wait_for(|&done| done).await;

        let cached =
            self.server.read().as_ref().map(|server| server.caps.get(C::static_name()).cloned());
        let Some(raw) = cached.ok_or(ServerSupportError::NotFetched)? else {
            return Ok(None);
        };

        serde_json::from_value(raw).map(Some).map_err(|source| ServerSupportError::Malformed {
            name: C::static_name(),
            source,
        })
    }

    /// The capability token this client currently knows, or `None` if it has never fetched.
    ///
    /// The token is opaque: it is the `version` of the server's capabilities document, echoed back
    /// to the server verbatim. The client never interprets it.
    #[must_use]
    pub fn known_token(&self) -> Option<String> {
        self.server.read().as_ref().map(|caps| caps.version.clone())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use atuin_domain::caps::{CapServer, CapabilitiesCap};
    use parking_lot::Mutex;
    use pretty_assertions::assert_eq;
    use reqwest::StatusCode;
    use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue};
    use rstest::rstest;
    use serde_json::json;
    use url::Url;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::{AuthHeaderProvider, CapClient, ServerSupportError};
    use crate::{ApiError, Client};

    const CAPABILITIES: &str = "/api/v0/capabilities";

    async fn server_answering(response: ResponseTemplate) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(CAPABILITIES))
            .respond_with(response)
            .mount(&server)
            .await;
        server
    }

    fn version(token: &str) -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_json(json!({"version": token, "capabilities": {}}))
    }

    fn api(server: &MockServer, http: reqwest::Client) -> Client {
        Client::from_http(&Url::parse(&server.uri()).unwrap(), http).unwrap()
    }

    /// A reader for `server` whose warm-up fetch has finished.
    async fn warm(server: &MockServer) -> Arc<CapClient> {
        let caps = CapClient::new(api(server, reqwest::Client::new()));
        let _ = caps.get_server::<CapabilitiesCap>().await;
        caps
    }

    async fn fetches(server: &MockServer) -> Vec<wiremock::Request> {
        server.received_requests().await.unwrap()
    }

    #[rstest]
    #[tokio::test]
    async fn known_token_is_the_version_of_the_last_fetch() {
        let server = server_answering(version("7")).await;
        let caps = CapClient::new(api(&server, reqwest::Client::new()));
        // The warm-up is spawned on this current-thread runtime and has not run yet.
        let before = caps.known_token();

        let _ = caps.get_server::<CapabilitiesCap>().await;

        assert_eq!((before, caps.known_token().as_deref()), (None, Some("7")));
    }

    #[rstest]
    #[tokio::test]
    async fn observes_the_capability_the_server_advertises() {
        let advertised = CapServer::new().add(CapabilitiesCap { version: 1 }).unwrap();
        let server = server_answering(
            ResponseTemplate::new(200).set_body_string(advertised.body().to_owned()),
        )
        .await;

        let caps = warm(&server).await;

        assert_eq!(
            caps.get_server::<CapabilitiesCap>().await.unwrap(),
            Some(CapabilitiesCap { version: 1 })
        );
    }

    /// A rejected fetch surfaces its status and leaves nothing cached, so negotiation-gated features
    /// see `NotFetched` rather than a stale "no".
    #[rstest]
    #[tokio::test]
    async fn a_rejected_fetch_leaves_the_capabilities_unfetched() {
        let server = server_answering(ResponseTemplate::new(401)).await;
        let caps = warm(&server).await;

        let err = caps.refresh().await.unwrap_err();

        assert!(
            matches!(err, ApiError::Status {
                status: StatusCode::UNAUTHORIZED,
                ..
            }),
            "{err:?}"
        );
        assert!(matches!(
            caps.get_server::<CapabilitiesCap>().await,
            Err(ServerSupportError::NotFetched)
        ));
    }

    #[rstest]
    #[case::current("7", 1)]
    #[case::stale("8", 2)]
    #[tokio::test]
    async fn refresh_if_stale_fetches_only_for_another_token(
        #[case] available: &str,
        #[case] expected_fetches: usize,
    ) {
        let server = server_answering(version("7")).await;
        let caps = warm(&server).await;

        caps.refresh_if_stale(available).await.unwrap();

        assert_eq!(fetches(&server).await.len(), expected_fetches);
    }

    /// Auth resolves on every fetch, so a long-lived reader follows login, logout and token
    /// rotation; it wins over a default `Authorization` (a user's extra header), which applies
    /// only while the provider has nothing.
    #[rstest]
    #[tokio::test]
    async fn resolves_authorization_on_every_fetch_over_the_default() {
        let server = server_answering(version("7")).await;
        let token = Arc::new(Mutex::new(Some("Token a")));
        let auth = AuthHeaderProvider::new({
            let token = Arc::clone(&token);
            move || {
                let value = token.lock().map(HeaderValue::from_static);
                Box::pin(async move { value })
            }
        });
        let http = reqwest::Client::builder()
            .default_headers(HeaderMap::from_iter([(
                AUTHORIZATION,
                HeaderValue::from_static("Basic proxy"),
            )]))
            .build()
            .unwrap();
        let caps = CapClient::new(api(&server, http).with_auth(auth));
        let _ = caps.get_server::<CapabilitiesCap>().await;

        *token.lock() = None;
        caps.refresh().await.unwrap();
        *token.lock() = Some("Token b");
        caps.refresh().await.unwrap();

        let sent: Vec<Vec<String>> = fetches(&server)
            .await
            .iter()
            .map(|request| {
                request
                    .headers
                    .get_all(AUTHORIZATION)
                    .iter()
                    .map(|value| value.to_str().unwrap().to_owned())
                    .collect()
            })
            .collect();
        assert_eq!(sent, [["Token a"], ["Basic proxy"], ["Token b"]]);
    }
}
