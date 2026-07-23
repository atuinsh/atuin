use std::{collections::HashMap, sync::Arc};

use url::Url;

use super::{CapKey, Capability, OwnCaps};
use crate::api::CapabilitiesResponse;
use crate::sync::CoalescingCell;

/// Client-side capability set: advertises its own capabilities and can read the server's.
///
/// The server's capabilities are populated by [`CapClient::refresh`], which the crate drives at
/// negotiation time; [`CapClient::server_support`] is then a pure, offline read of that cache.
///
/// Cloning is cheap.
#[derive(Debug, Clone)]
pub struct CapClient {
    inner: Arc<CapClientInner>,
}

#[derive(Debug)]
struct CapClientInner {
    /// This client's own capabilities.
    own: OwnCaps,
    /// The server's capabilities. Concurrent `refresh` calls coalesce into a single network hop.
    server: CoalescingCell<ServerCaps>,
    /// The server's capabilities endpoint. Passed in by the caller so this crate stays agnostic of
    /// the route (eg `/api/v0/capabilities`).
    capabilities_url: Url,
}

/// The capabilities a server advertises, as last fetched from its capabilities endpoint.
#[derive(Debug)]
struct ServerCaps {
    /// The server's capability version.
    version: usize,
    caps: HashMap<CapKey, serde_json::Value>,
}

impl From<CapabilitiesResponse> for ServerCaps {
    fn from(resp: CapabilitiesResponse) -> Self {
        Self {
            version: resp.version,
            caps: resp
                .capabilities
                .into_iter()
                .map(|(name, value)| (CapKey(name), value))
                .collect(),
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
    /// Create a client that will negotiate against the given capabilities endpoint.
    pub fn new(capabilities_url: Url) -> Self {
        Self {
            inner: Arc::new(CapClientInner {
                own: OwnCaps::default(),
                server: CoalescingCell::default(),
                capabilities_url,
            }),
        }
    }

    /// Register a capability this client advertises.
    pub fn can<C: Capability>(&self, cap: C) {
        self.inner.own.can(cap);
    }

    /// Check whether this client advertises the given capability.
    pub fn support<C: Capability + Clone>(&self) -> Option<C> {
        self.inner.own.support()
    }

    /// Fetch the server's capabilities over the caller's client and patch the local cache.
    ///
    /// It is safe to call this in parallel, even under high load.
    pub(crate) async fn refresh(&self, client: &reqwest::Client) -> reqwest::Result<()> {
        self.inner
            .server
            .refresh(|| async {
                let resp: CapabilitiesResponse = client
                    .get(self.inner.capabilities_url.clone())
                    .send()
                    .await?
                    .json()
                    .await?;
                Ok(ServerCaps::from(resp))
            })
            .await?;

        Ok(())
    }

    /// Read whether the server supports the given capability, from the last [`CapClient::refresh`].
    ///
    /// - `Ok(Some(c))` - the server advertises the capability and it deserialized into `C`.
    /// - `Ok(None)` - fetched, and the server does not advertise it (a definitive "no").
    /// - `Err(ServerSupportError::NotFetched)` - capabilities have not been fetched yet.
    /// - `Err(ServerSupportError::Malformed)` - advertised, but its value did not deserialize into
    ///   `C`. The caller decides whether that is fatal or a reason to fall back.
    pub fn server_support<C: Capability>(&self) -> Result<Option<C>, ServerSupportError> {
        let Some(server) = self.inner.server.get() else {
            return Err(ServerSupportError::NotFetched);
        };
        let Some(raw) = server.caps.get(C::NAME) else {
            return Ok(None);
        };

        serde_json::from_value(raw.clone())
            .map(Some)
            .map_err(|source| ServerSupportError::Malformed {
                name: C::NAME,
                source,
            })
    }

    /// The capability token this client currently knows, or `None` if it has never fetched.
    ///
    /// The token is opaque: it is the server's [`ServerCaps::version`] stringified, echoed back to
    /// the server verbatim. The client never interprets it.
    pub(crate) fn known_token(&self) -> Option<String> {
        self.inner.server.get().map(|caps| caps.version.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn known_token_reflects_the_last_refresh() {
        crate::tls::ensure_crypto_provider();

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v0/capabilities"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "version": 7,
                "capabilities": {}
            })))
            .mount(&server)
            .await;

        let caps_url: Url = format!("{}/api/v0/capabilities", server.uri())
            .parse()
            .unwrap();
        let client = CapClient::new(caps_url);

        // Nothing fetched yet.
        assert_eq!(client.known_token(), None);

        client.refresh(&reqwest::Client::new()).await.unwrap();

        // The token is the server's version, stringified and opaque.
        assert_eq!(client.known_token(), Some("7".to_string()));
    }
}
