use std::sync::Arc;

use atuin_domain::caps::CapClient;
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

        Ok(SyncEngine {
            client,
            store: self.store,
            page_size: DEFAULT_PAGE_SIZE,
        })
    }
}

impl SyncEngine {
    /// Start building a [`SyncEngine`]. See [`SyncEngineInit`] for the construction paths.
    pub fn builder<'a>() -> SyncEngineBuilder<'a, ((), ())> {
        SyncEngineInit::builder()
    }
}
