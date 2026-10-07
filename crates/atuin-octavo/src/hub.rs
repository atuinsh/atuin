use std::time::Duration;

use atuin_client::history::{CommandCapture, History, HistoryId};
use atuin_client::settings::{DEFAULT_HUB_URL, DEFAULT_SYNC_URL, Settings, SyncAuth};
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
    #[error("Octavo runs only on Atuin's hosted hub, not {0}")]
    SelfHostedHub(Url),
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
            | Self::SelfHostedHub(_)
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

/// Atuin's hosted hub, which serves Octavo at `api.atuin.sh`.
#[derive(Debug, Clone)]
pub struct HubClient {
    service: HubServiceClient<Channel>,
}

impl HubClient {
    pub fn new(settings: &Settings) -> Result<Self, ConnectError> {
        let channel = connect(&DEFAULT_SYNC_URL, settings.network_connect_timeout)?;
        Ok(Self {
            service: HubServiceClient::new(channel),
        })
    }

    /// The hub token logged in now, so work done for this moment's user later still goes to them
    /// after another login.
    pub async fn token(&self, settings: &Settings) -> Result<HubToken, HubCallError> {
        // The hub session belongs to whichever hub is logged in, and a self-hosted hub's token
        // must never reach Atuin's.
        let hub = settings.hub_endpoint();
        if hub != *DEFAULT_HUB_URL {
            return Err(HubCallError::SelfHostedHub(hub));
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

        let user_id = match Self::known_user(&token).await? {
            Some(id) => id,
            None => {
                let id = caller.who_am_i().await?;
                Settings::meta_store()
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
    ) -> Result<Option<UserId>, HubCallError> {
        Self::known_user(token).await
    }

    async fn known_user(token: &SecretString) -> Result<Option<UserId>, HubCallError> {
        let meta = Settings::meta_store().await.map_err(HubCallError::Meta)?;
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

#[cfg(test)]
mod tests {
    use atuin_client::settings::{Settings, SyncProtocol};
    use rstest::rstest;
    use url::Url;

    use super::{HubCallError, HubClient};

    #[rstest]
    #[tokio::test]
    async fn self_hosted_hub_token_stays_home() {
        let settings = Settings {
            sync_address: Url::parse("http://localhost:4000").unwrap(),
            sync_protocol: SyncProtocol::Hub,
            ..Settings::default()
        };

        let token = HubClient::new(&settings).unwrap().token(&settings).await;

        assert!(matches!(token, Err(HubCallError::SelfHostedHub(_))));
    }
}
