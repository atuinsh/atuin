use std::time::Duration;

use atuin_client::history::{CommandCapture, History, HistoryId};
use atuin_client::settings::{Settings, SyncAuth};
use atuin_domain::record::RecordId;
use hyper_tls::HttpsConnector;
use hyper_util::client::legacy::connect::HttpConnector;
use secrecy::{ExposeSecret, SecretString};
use thiserror::Error;
use tonic::metadata::errors::InvalidMetadataValue;
use tonic::metadata::{Ascii, MetadataValue};
use tonic::transport::{Channel, Endpoint};
use tonic::{Code, Request};
use url::Url;

use crate::pb;
use crate::pb::hub_service_client::HubServiceClient;
use crate::pb::{HistoryConversionError, NotUuidV7, OutputTooLarge};

#[derive(Debug, Error)]
pub enum ConnectError {
    #[error("the Octavo endpoint {address} is not a valid URI")]
    Endpoint {
        address: Url,
        #[source]
        source: tonic::transport::Error,
    },
    #[error("failed to set up TLS")]
    Tls(#[source] native_tls::Error),
}

#[derive(Debug, Error)]
pub enum HubCallError {
    #[error(transparent)]
    Convert(#[from] HistoryConversionError),
    #[error(transparent)]
    Id(#[from] NotUuidV7),
    #[error(transparent)]
    OutputTooLarge(#[from] OutputTooLarge),
    #[error("the hub has not stored the history entry yet")]
    HistoryNotStored(#[source] tonic::Status),
    #[error("Octavo needs a hub login; run `atuin login`")]
    NotLoggedIn,
    #[error("failed to read the hub login: {0:#}")]
    Meta(eyre::Report),
    #[error(transparent)]
    Connect(#[from] ConnectError),
    #[error("the Octavo endpoint is now {0}; restart the daemon to use it")]
    EndpointChanged(Url),
    #[error("the hub token is not a valid header value")]
    InvalidToken(#[source] InvalidMetadataValue),
    #[error("the hub named no user for the token")]
    NoUserId,
    #[error("the hub refused the call")]
    Hub(#[source] tonic::Status),
}

impl HubCallError {
    /// Whether the call would fail the same way however often it's retried.
    #[must_use]
    pub fn is_permanent(&self) -> bool {
        match self {
            Self::Convert(_) | Self::Id(_) | Self::OutputTooLarge(_) => true,
            Self::Hub(status) => status.code() == Code::InvalidArgument,
            Self::NotLoggedIn
            | Self::Meta(_)
            | Self::Connect(_)
            | Self::EndpointChanged(_)
            | Self::InvalidToken(_)
            | Self::HistoryNotStored(_)
            | Self::NoUserId => false,
        }
    }
}

/// A hub user's id, as `WhoAmI` names it. Opaque.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserId(String);

impl UserId {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
impl UserId {
    pub(crate) fn new(id: &str) -> Self {
        Self(id.to_owned())
    }
}

/// The hub Octavo's settings pointed at when the client was made.
#[derive(Debug, Clone)]
pub struct HubClient {
    address: Url,
    service: HubServiceClient<Channel>,
}

impl HubClient {
    pub fn new(settings: &Settings) -> Result<Self, ConnectError> {
        let address = octavo_address(settings);
        let service = HubServiceClient::new(connect(&address, settings.network_connect_timeout)?);
        Ok(Self { address, service })
    }

    /// The hub token logged in now, so work done for this moment's user later still goes to them
    /// after another login.
    pub async fn token(&self, settings: &Settings) -> Result<HubToken, HubCallError> {
        // A token for one hub must never reach another, so a client keeps to the hub it was made
        // for.
        let configured = octavo_address(settings);
        if configured != self.address {
            return Err(HubCallError::EndpointChanged(configured));
        }

        let SyncAuth::Hub { token } = settings.resolve_sync_auth().await else {
            return Err(HubCallError::NotLoggedIn);
        };
        Ok(HubToken(token))
    }

    /// Logs in with the hub token logged in now.
    pub async fn login(&self, settings: &Settings) -> Result<HubLogin, HubCallError> {
        let token = self.token(settings).await?;
        self.login_as(token, settings).await
    }

    /// Logs in with `token`, asking the hub whose it is the first time it's used, and remembering
    /// the answer for as long as it's the hub session.
    pub async fn login_as(
        &self,
        HubToken(token): HubToken,
        settings: &Settings,
    ) -> Result<HubLogin, HubCallError> {
        let mut authorization =
            MetadataValue::try_from(format!("Bearer {}", token.expose_secret()))
                .map_err(HubCallError::InvalidToken)?;
        authorization.set_sensitive(true);
        let caller = Caller {
            service: self.service.clone(),
            authorization,
            timeout: settings.network_timeout,
        };

        let user_id = match Self::known_user(&token, settings).await? {
            Some(id) => id,
            None => {
                let id = caller.who_am_i().await?;
                settings
                    .meta_store()
                    .await
                    .map_err(HubCallError::Meta)?
                    .save_hub_user_id(&token, id.as_str())
                    .await
                    .map_err(HubCallError::Meta)?;
                id
            }
        };

        Ok(HubLogin { caller, user_id })
    }

    /// The user `token` belongs to, if a login with it already learned that while it's been the
    /// hub session. Never calls the hub.
    pub async fn user_of(
        &self,
        HubToken(token): &HubToken,
        settings: &Settings,
    ) -> Result<Option<UserId>, HubCallError> {
        Self::known_user(token, settings).await
    }

    async fn known_user(
        token: &SecretString,
        settings: &Settings,
    ) -> Result<Option<UserId>, HubCallError> {
        let meta = settings.meta_store().await.map_err(HubCallError::Meta)?;
        let id = meta.hub_user_id(token).await.map_err(HubCallError::Meta)?;
        Ok(id.map(UserId))
    }
}

/// A hub token, as the hub session held it when [`HubClient::token`] read it.
#[derive(Debug)]
pub struct HubToken(SecretString);

/// The hub, as the user a hub token belongs to.
#[derive(Debug)]
pub struct HubLogin {
    caller: Caller,
    user_id: UserId,
}

impl HubLogin {
    pub fn user_id(&self) -> &UserId {
        &self.user_id
    }

    pub async fn upload(
        &self,
        history: History,
        record_id: Option<RecordId>,
    ) -> Result<(), HubCallError> {
        let mut history = pb::History::try_from(history)?;
        history.record_id = record_id.map(pb::UuidV7::from);
        let request = self.caller.request(pb::InsertHistoryRequest {
            history: Some(history),
        });

        match self.caller.service.clone().insert_history(request).await {
            Ok(_) => Ok(()),
            Err(status) if status.code() == Code::AlreadyExists => Ok(()),
            Err(status) => Err(HubCallError::Hub(status)),
        }
    }

    /// Stores `capture` as the output of the entry with `id`.
    ///
    /// Fails with [`HubCallError::HistoryNotStored`] until the entry itself is stored.
    pub async fn upload_output(
        &self,
        id: HistoryId,
        capture: CommandCapture,
    ) -> Result<(), HubCallError> {
        let request = self.caller.request(pb::InsertHistoryOutputRequest {
            id: Some(id.try_into()?),
            output: Some(capture.try_into()?),
        });

        match self.caller.service.clone().insert_history_output(request).await {
            Ok(_) => Ok(()),
            Err(status) if status.code() == Code::AlreadyExists => Ok(()),
            Err(status) if status.code() == Code::NotFound => {
                Err(HubCallError::HistoryNotStored(status))
            }
            Err(status) => Err(HubCallError::Hub(status)),
        }
    }

    /// Deletes the entry with `id`, and keeps it from being stored again.
    pub async fn delete(&self, id: HistoryId) -> Result<(), HubCallError> {
        let request = self.caller.request(pb::DeleteHistoryRequest {
            id: Some(id.try_into()?),
        });

        self.caller.service.clone().delete_history(request).await.map_err(HubCallError::Hub)?;
        Ok(())
    }
}

#[derive(Debug)]
struct Caller {
    service: HubServiceClient<Channel>,
    authorization: MetadataValue<Ascii>,
    timeout: Duration,
}

impl Caller {
    fn request<T>(&self, message: T) -> Request<T> {
        let mut request = Request::new(message);
        request.set_timeout(self.timeout);
        request.metadata_mut().insert("authorization", self.authorization.clone());
        request
    }

    async fn who_am_i(&self) -> Result<UserId, HubCallError> {
        let request = self.request(pb::WhoAmIRequest {});
        let user_id =
            self.service.clone().who_am_i(request).await.map_err(HubCallError::Hub)?.into_inner();

        if user_id.user_id.is_empty() {
            return Err(HubCallError::NoUserId);
        }
        Ok(UserId(user_id.user_id))
    }
}

fn octavo_address(settings: &Settings) -> Url {
    settings.octavo.endpoint.clone().unwrap_or_else(|| settings.hub_endpoint())
}

fn connect(address: &Url, connect_timeout: Duration) -> Result<Channel, ConnectError> {
    // Together under the default 30s `network_timeout`, so a call on a dead connection drops it,
    // and the retry reconnects instead of waiting out the timeout on the same connection.
    const KEEP_ALIVE_INTERVAL: Duration = Duration::from_secs(10);
    const KEEP_ALIVE_TIMEOUT: Duration = Duration::from_secs(10);

    let endpoint = Endpoint::from_shared(address.to_string())
        .map_err(|source| ConnectError::Endpoint {
            address: address.clone(),
            source,
        })?
        .connect_timeout(connect_timeout)
        .http2_keep_alive_interval(KEEP_ALIVE_INTERVAL)
        .keep_alive_timeout(KEEP_ALIVE_TIMEOUT);

    Ok(endpoint.connect_with_connector_lazy(connector()?))
}

fn connector() -> Result<HttpsConnector<HttpConnector>, ConnectError> {
    let mut http = HttpConnector::new();
    http.enforce_http(false);
    let tls = native_tls::TlsConnector::builder()
        .request_alpns(&["h2"])
        .build()
        .map_err(ConnectError::Tls)?;
    Ok(HttpsConnector::from((http, tls.into())))
}
