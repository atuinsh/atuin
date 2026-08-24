use std::num::NonZeroU64;
use std::sync::Arc;

use atuin_domain::caps::{CapClient, PageSizeCap};
use eyre::Result;
use tracing::instrument;
use typed_builder::TypedBuilder;

use super::{DEFAULT_PAGE_SIZE, SyncEngine, SyncError};
use crate::api_client::{Client, caps_client};
use crate::record::sqlite_store::SqliteStore;
use crate::settings::Settings;

/// Where a [`SyncEngine`]'s API client comes from.
pub enum ClientSource<'a> {
    /// Wrap an already-built [`Client`].
    FromClient(Client),
    /// Build from settings, fetching capabilities during `connect`, unless supplied here.
    FromSettings {
        settings: &'a Settings,
        caps: Option<Arc<CapClient>>,
    },
}

/// Inputs for constructing a [`SyncEngine`]. See [`SyncEngine::builder`].
#[derive(TypedBuilder)]
#[builder(builder_type(name = SyncEngineBuilder), builder_method(vis = "pub(crate)"))]
pub struct SyncEngineInit<'a> {
    store: SqliteStore,
    client_source: ClientSource<'a>,
}

impl SyncEngineInit<'_> {
    /// Resolve the configured inputs into a live [`SyncEngine`].
    #[instrument(level = "trace", skip_all, err)]
    pub async fn connect(self) -> Result<SyncEngine, SyncError> {
        let client = match self.client_source {
            ClientSource::FromClient(client) => client,
            ClientSource::FromSettings { settings, caps } => {
                let caps = match caps {
                    Some(caps) => caps,
                    None => caps_client(&settings.sync_address, &settings.extra_headers)
                        .map_err(|e| SyncError::OperationalError { msg: e.to_string() })?,
                };

                Client::new(
                    settings.sync_address.clone(),
                    &settings
                        .sync_auth_token()
                        .await
                        .map_err(|e| SyncError::RemoteRequestError { msg: e.to_string() })?,
                    settings.network_connect_timeout,
                    settings.network_timeout,
                    &settings.extra_headers,
                    caps,
                )
                .map_err(|e| SyncError::OperationalError { msg: e.to_string() })?
            }
        };

        let page_size = negotiate_page_size(client.caps()).await;

        Ok(SyncEngine {
            client,
            store: self.store,
            page_size,
        })
    }
}

impl SyncEngine {
    /// Start building a [`SyncEngine`]. See [`SyncEngineInit`] for the construction paths.
    pub fn builder<'a>() -> SyncEngineBuilder<'a, ((), ())> {
        SyncEngineInit::builder()
    }
}

async fn negotiate_page_size(caps: &CapClient) -> NonZeroU64 {
    caps.get_server::<PageSizeCap>()
        .await
        .ok()
        .flatten()
        .and_then(|cap| NonZeroU64::new(cap.page_size))
        .unwrap_or(DEFAULT_PAGE_SIZE)
}

#[cfg(test)]
mod page_size_negotiation_tests {
    use std::num::NonZeroU64;

    use atuin_domain::caps::{CapClient, CapServer, CapabilitiesCap, PageSizeCap};
    use url::Url;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::{DEFAULT_PAGE_SIZE, negotiate_page_size};

    async fn cap_client_serving(body: String) -> std::sync::Arc<CapClient> {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v0/capabilities"))
            .respond_with(ResponseTemplate::new(200).set_body_string(body))
            .mount(&server)
            .await;
        let caps_url: Url = format!("{}/api/v0/capabilities", server.uri()).parse().unwrap();
        let client = CapClient::new(caps_url, reqwest::Client::new());
        Box::leak(Box::new(server));
        client
    }

    #[tokio::test]
    async fn uses_the_advertised_page_size() {
        let caps = CapServer::new()
            .add(CapabilitiesCap { version: 1 })
            .unwrap()
            .add(PageSizeCap {
                version: 1,
                page_size: 250,
            })
            .unwrap();
        let client = cap_client_serving(caps.body().to_owned()).await;

        assert_eq!(negotiate_page_size(&client).await, NonZeroU64::new(250).unwrap());
    }

    #[tokio::test]
    async fn falls_back_when_cap_absent() {
        let caps = CapServer::new().add(CapabilitiesCap { version: 1 }).unwrap();
        let client = cap_client_serving(caps.body().to_owned()).await;

        assert_eq!(negotiate_page_size(&client).await, DEFAULT_PAGE_SIZE);
    }

    #[tokio::test]
    async fn falls_back_when_page_size_is_zero() {
        let caps = CapServer::new()
            .add(CapabilitiesCap { version: 1 })
            .unwrap()
            .add(PageSizeCap {
                version: 1,
                page_size: 0,
            })
            .unwrap();
        let client = cap_client_serving(caps.body().to_owned()).await;

        assert_eq!(negotiate_page_size(&client).await, DEFAULT_PAGE_SIZE);
    }
}
