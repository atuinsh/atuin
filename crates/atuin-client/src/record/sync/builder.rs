use std::sync::Arc;

use atuin_domain::caps::CapClient;
use eyre::Result;
use tracing::instrument;
use typed_builder::TypedBuilder;

use super::{SyncError, SyncSession};
use crate::api_client::{Client, caps_client};
use crate::record::sqlite_store::SqliteStore;
use crate::settings::Settings;

/// Where a [`SyncSession`]'s API client comes from.
pub enum ClientSource<'a> {
    /// Wrap an already-built [`Client`].
    FromClient(Client),
    /// Build from settings, fetching capabilities during `connect`, unless supplied here.
    FromSettings {
        settings: &'a Settings,
        caps: Option<Arc<CapClient>>,
    },
}

/// Inputs for constructing a [`SyncSession`]. See [`SyncSession::builder`].
#[derive(TypedBuilder)]
#[builder(builder_type(name = SyncSessionBuilder), builder_method(vis = "pub(crate)"))]
pub struct SyncSessionInit<'a> {
    store: SqliteStore,
    client_source: ClientSource<'a>,
}

impl SyncSessionInit<'_> {
    /// Resolve the configured inputs into a live [`SyncSession`].
    #[instrument(level = "trace", skip_all, err)]
    pub async fn connect(self) -> Result<SyncSession, SyncError> {
        let client = match self.client_source {
            ClientSource::FromClient(client) => client,
            ClientSource::FromSettings { settings, caps } => {
                let auth = settings
                    .sync_auth_token()
                    .await
                    .map_err(|e| SyncError::RemoteRequestError { msg: e.to_string() })?;

                let caps = match caps {
                    Some(caps) => caps,
                    None => caps_client(settings)
                        .map_err(|e| SyncError::OperationalError { msg: e.to_string() })?,
                };

                Client::new(
                    settings.sync_address.clone(),
                    &auth,
                    settings.network_connect_timeout,
                    settings.network_timeout,
                    &settings.extra_headers,
                    caps,
                )
                .map_err(|e| SyncError::OperationalError { msg: e.to_string() })?
            }
        };

        Ok(SyncSession {
            client,
            store: self.store,
            page_size_override: None,
        })
    }
}

impl SyncSession {
    /// Start building a [`SyncSession`]. See [`SyncSessionInit`] for the construction paths.
    pub fn builder<'a>() -> SyncSessionBuilder<'a, ((), ())> {
        SyncSessionInit::builder()
    }
}
