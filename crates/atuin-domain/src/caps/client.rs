use super::{CapKey, Capability, CapsBundle, DuplicateCapability};
use crate::api::CapabilitiesResponse;
use parking_lot::RwLock;
use serde::de::DeserializeOwned;
use std::{collections::HashMap, sync::Arc};
use tokio;
use url::Url;

/// Client-side capability set: advertises its own capabilities and can read the server's.
///
/// The server's capabilities are populated by [`CapClient::refresh`], which the crate drives at
/// negotiation time; [`CapClient::get_server`] is then a pure, offline read of that cache.
///
/// Thread it as an [`Arc`].
#[derive(Debug)]
pub struct CapClient {
    /// This client's own capabilities.
    own: CapsBundle,
    /// The server's capabilities as last fetched; `None` until the first refresh. Cheap concurrent
    /// reads; writes are serialized by `fetching`.
    server: RwLock<Option<ServerCaps>>,
    /// Serializes capability fetches so a burst of stale callers makes a single network hop.
    fetching: tokio::sync::Mutex<()>,
    /// The server's capabilities endpoint. Passed in by the caller so this crate stays agnostic of
    /// the route (eg `/api/v0/capabilities`).
    capabilities_url: Url,
    /// Bare client used to fetch `capabilities_url`.
    http: reqwest::Client,
    warmed: tokio::sync::watch::Receiver<bool>,
}

/// The capabilities a server advertises, as last fetched from its capabilities endpoint.
#[derive(Debug)]
struct ServerCaps {
    /// The server's capability version.
    version: String,
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
    pub fn new(capabilities_url: Url, http: reqwest::Client) -> Arc<Self> {
        let (tx, rx) = tokio::sync::watch::channel(false);
        let new = Arc::new(Self {
            own: CapsBundle::default(),
            server: RwLock::new(None),
            fetching: tokio::sync::Mutex::new(()),
            capabilities_url,
            http,
            warmed: rx,
        });

        let this = new.clone();
        tokio::spawn(async move {
            let _ = this.refresh().await;
            let _ = tx.send(true);
        });

        new
    }

    /// Register a capability this client advertises.
    ///
    /// Errors with [`DuplicateCapability`] if a capability with the same name is already advertised.
    pub fn add<C: Capability>(&self, cap: C) -> Result<(), DuplicateCapability> {
        self.own.add(cap)
    }

    /// Check whether this client advertises the given capability.
    pub fn get<C: Capability + Clone>(&self) -> Option<C> {
        self.own.get()
    }

    /// Fetch the server's capabilities over the internal client and patch the local cache.
    ///
    /// Fetches are serialized so parallel callers never overlap; this one always fetches.
    pub async fn refresh(&self) -> reqwest::Result<()> {
        let _fetching = self.fetching.lock().await;
        let caps = self.fetch_server_caps().await?;
        *self.server.write() = Some(caps);
        Ok(())
    }

    /// Refresh the server's capabilities only if our known token differs from `available`.
    ///
    /// Double-checked so a burst of stale callers makes a single fetch, and a caller whose token
    /// already matches `available` does no work.
    pub async fn refresh_if_stale(&self, available: &str) -> reqwest::Result<()> {
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

    /// Whether our cached server token differs from `available` (or we have not fetched yet).
    fn is_stale(&self, available: &str) -> bool {
        self.server
            .read()
            .as_ref()
            .map(|caps| caps.version.as_str())
            != Some(available)
    }

    /// Fetch and decode the server's capabilities document.
    async fn fetch_server_caps(&self) -> reqwest::Result<ServerCaps> {
        let resp: CapabilitiesResponse = self
            .http
            .get(self.capabilities_url.clone())
            .send()
            .await?
            .json()
            .await?;
        Ok(ServerCaps::from(resp))
    }

    /// Read whether the server supports the given capability, from the last [`CapClient::refresh`].
    ///
    /// - `Ok(Some(c))` - the server advertises the capability and it deserialized into `C`.
    /// - `Ok(None)` - fetched, and the server does not advertise it (a definitive "no").
    /// - `Err(ServerSupportError::NotFetched)` - capabilities have not been fetched yet.
    /// - `Err(ServerSupportError::Malformed)` - advertised, but its value did not deserialize into
    ///   `C`. The caller decides whether that is fatal or a reason to fall back.
    pub async fn get_server<C: Capability + DeserializeOwned>(
        &self,
    ) -> Result<Option<C>, ServerSupportError> {
        let _ = self.warmed.clone().wait_for(|&done| done).await;

        let server = self.server.read();
        let Some(server) = server.as_ref() else {
            return Err(ServerSupportError::NotFetched);
        };
        let Some(raw) = server.caps.get(C::static_name()) else {
            return Ok(None);
        };

        serde_json::from_value(raw.clone())
            .map(Some)
            .map_err(|source| ServerSupportError::Malformed {
                name: C::static_name(),
                source,
            })
    }

    /// Whether the server's capabilities have been fetched at least once.
    pub fn is_fetched(&self) -> bool {
        self.server.read().is_some()
    }

    /// The capability token this client currently knows, or `None` if it has never fetched.
    ///
    /// The token is opaque: it is the server's [`ServerCaps::version`], echoed back to the server
    /// verbatim. The client never interprets it.
    pub(crate) fn known_token(&self) -> Option<String> {
        self.server.read().as_ref().map(|caps| caps.version.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::caps::{CapServer, CapabilitiesCap};
    use rstest::{fixture, rstest};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// A plain reqwest client for the network tests.
    #[fixture]
    fn http_client() -> reqwest::Client {
        reqwest::Client::new()
    }

    #[rstest]
    #[tokio::test]
    async fn known_token_reflects_the_last_refresh(http_client: reqwest::Client) {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v0/capabilities"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "version": "7",
                "capabilities": {}
            })))
            .mount(&server)
            .await;

        let caps_url: Url = format!("{}/api/v0/capabilities", server.uri())
            .parse()
            .unwrap();
        let client = CapClient::new(caps_url, http_client);

        // Nothing fetched yet.
        assert_eq!(client.known_token(), None);

        client.refresh().await.unwrap();

        // The token is the server's version, opaque and echoed verbatim.
        assert_eq!(client.known_token(), Some("7".to_string()));
    }

    #[rstest]
    #[tokio::test]
    async fn client_observes_the_capability_the_server_advertises(http_client: reqwest::Client) {
        // Serve the exact wire body a real server would produce for the capabilities capability.
        let advertised = CapServer::new()
            .add(CapabilitiesCap { version: 1 })
            .unwrap();
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v0/capabilities"))
            .respond_with(ResponseTemplate::new(200).set_body_string(advertised.body().to_owned()))
            .mount(&server)
            .await;

        let caps_url: Url = format!("{}/api/v0/capabilities", server.uri())
            .parse()
            .unwrap();
        let client = CapClient::new(caps_url, http_client);

        assert_eq!(
            client.get_server::<CapabilitiesCap>().await.unwrap(),
            Some(CapabilitiesCap { version: 1 })
        );
    }
}
