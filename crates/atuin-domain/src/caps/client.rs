use std::{collections::HashMap, sync::Arc};

use serde::de::DeserializeOwned;
use url::Url;

use super::{CapKey, Capability, CapsBundle};
use crate::api::CapabilitiesResponse;
use atuin_common::sync::SingleFlight;

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
    /// The server's capabilities. Concurrent `refresh` calls coalesce into a single network hop.
    server: SingleFlight<ServerCaps>,
    /// The server's capabilities endpoint. Passed in by the caller so this crate stays agnostic of
    /// the route (eg `/api/v0/capabilities`).
    capabilities_url: Url,
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
    pub fn new(capabilities_url: Url) -> Arc<Self> {
        Arc::new(Self {
            own: CapsBundle::default(),
            server: SingleFlight::default(),
            capabilities_url,
        })
    }

    /// Register a capability this client advertises.
    pub fn add<C: Capability>(&self, cap: C) {
        self.own.add(cap);
    }

    /// Check whether this client advertises the given capability.
    pub fn get<C: Capability + Clone>(&self) -> Option<C> {
        self.own.get()
    }

    /// Fetch the server's capabilities over the caller's client and patch the local cache.
    ///
    /// It is safe to call this in parallel, even under high load.
    pub async fn refresh(&self, client: &reqwest::Client) -> reqwest::Result<()> {
        self.server
            .refresh(|| async {
                let resp: CapabilitiesResponse = client
                    .get(self.capabilities_url.clone())
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
    pub fn get_server<C: Capability + DeserializeOwned>(
        &self,
    ) -> Result<Option<C>, ServerSupportError> {
        let Some(server) = self.server.get() else {
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

    /// The capability token this client currently knows, or `None` if it has never fetched.
    ///
    /// The token is opaque: it is the server's [`ServerCaps::version`], echoed back to the server
    /// verbatim. The client never interprets it.
    pub(crate) fn known_token(&self) -> Option<String> {
        self.server.get().map(|caps| caps.version.clone())
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
        let client = CapClient::new(caps_url);

        // Nothing fetched yet.
        assert_eq!(client.known_token(), None);

        client.refresh(&http_client).await.unwrap();

        // The token is the server's version, opaque and echoed verbatim.
        assert_eq!(client.known_token(), Some("7".to_string()));
    }

    #[rstest]
    #[tokio::test]
    async fn client_observes_the_capability_the_server_advertises(http_client: reqwest::Client) {
        // Serve the exact wire body a real server would produce for the capabilities capability.
        let advertised = CapServer::new().add(CapabilitiesCap { version: 1 });
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v0/capabilities"))
            .respond_with(ResponseTemplate::new(200).set_body_string(advertised.body().to_owned()))
            .mount(&server)
            .await;

        let caps_url: Url = format!("{}/api/v0/capabilities", server.uri())
            .parse()
            .unwrap();
        let client = CapClient::new(caps_url);

        // Nothing fetched yet, so the capability cannot be observed.
        assert!(matches!(
            client.get_server::<CapabilitiesCap>(),
            Err(ServerSupportError::NotFetched)
        ));

        client.refresh(&http_client).await.unwrap();

        // Having refreshed, the client observes the capability the server advertised.
        assert_eq!(
            client.get_server::<CapabilitiesCap>().unwrap(),
            Some(CapabilitiesCap { version: 1 })
        );
    }
}
