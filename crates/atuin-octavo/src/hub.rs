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
use tracing::warn;
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
    #[error("history {0}")]
    Id(#[from] NotUuidV7<HistoryId>),
    #[error("record {0}")]
    RecordId(#[from] NotUuidV7<RecordId>),
    #[error(transparent)]
    OutputTooLarge(#[from] OutputTooLarge),
    #[error("the hub has not stored the history entry yet")]
    HistoryNotStored(#[source] tonic::Status),
    #[error("Octavo needs a hub login; run `atuin login`")]
    NotLoggedIn,
    #[error("the hub login was saved without its user, which the next upload pass learns")]
    UnknownUser,
    #[error("failed to read the hub login: {0:#}")]
    Meta(eyre::Report),
    #[error(transparent)]
    Connect(#[from] ConnectError),
    #[error("Octavo runs only on Atuin's hosted hub, not {0}")]
    SelfHostedHub(Url),
    #[error("the hub changed to {0} after Octavo connected; restart atuin")]
    HubChanged(Url),
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
            Self::Convert(_) | Self::Id(_) | Self::RecordId(_) | Self::OutputTooLarge(_) => true,
            Self::Hub(status) => status.code() == Code::InvalidArgument,
            Self::NotLoggedIn
            | Self::UnknownUser
            | Self::Meta(_)
            | Self::Connect(_)
            | Self::SelfHostedHub(_)
            | Self::HubChanged(_)
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

/// Where a [`HubClient`] uploads, decided once when it is built.
#[derive(Debug, Clone)]
enum Target {
    /// Atuin's hosted hub.
    Official,
    /// A self-hosted hub, which Octavo refuses: there is nowhere to upload to.
    SelfHosted,
    /// The `octavo.__debug__.address` of a self-hosted hub.
    Debug(Url),
}

impl Target {
    fn of(settings: &Settings) -> Self {
        if settings.hub_endpoint() == *DEFAULT_HUB_URL {
            return Self::Official;
        }
        match &settings.octavo.debug.address {
            Some(address) => Self::Debug(address.clone()),
            None => Self::SelfHosted,
        }
    }

    fn address(&self) -> &Url {
        match self {
            Self::Official | Self::SelfHosted => &DEFAULT_SYNC_URL,
            Self::Debug(address) => address,
        }
    }
}

/// The hub Octavo uploads to: Atuin's hosted hub at `api.atuin.sh`, or the debug address of a
/// self-hosted hub.
///
/// The hub and address are fixed when it is built, so a settings reload that points at another
/// hub fails with [`HubCallError::HubChanged`] until atuin restarts.
#[derive(Debug, Clone)]
pub struct HubClient {
    service: HubServiceClient<Channel>,
    hub: Url,
    target: Target,
}

impl HubClient {
    pub fn new(settings: &Settings) -> Result<Self, ConnectError> {
        let target = Target::of(settings);
        if settings.octavo.debug.address.is_some() && matches!(target, Target::Official) {
            warn!(
                "octavo.__debug__.address is honored only while sync_address is a self-hosted hub \
                 (sync_protocol = \"hub\"); uploading to Atuin's hub"
            );
        }
        let channel = connect(target.address(), settings.network_connect_timeout)?;
        Ok(Self {
            service: HubServiceClient::new(channel),
            hub: settings.hub_endpoint(),
            target,
        })
    }

    /// The hub session logged in now, so work done for this moment's user later still goes to them
    /// after another login. Never calls the hub.
    ///
    /// Fails with [`HubCallError::UnknownUser`] for a session saved without its user, by a login
    /// from before atuin stored it, until [`Self::login`] learns whose it is.
    pub async fn session(&self, settings: &Settings) -> Result<HubSession, HubCallError> {
        let token = self.token(settings).await?;
        let user_id = Self::known_user(&token, settings).await?.ok_or(HubCallError::UnknownUser)?;
        Ok(HubSession { token, user_id })
    }

    /// Logs in with the hub session logged in now. For a session saved without its user, asks the
    /// hub whose it is, and remembers the answer for as long as it's the hub session.
    pub async fn login(&self, settings: &Settings) -> Result<HubLogin, HubCallError> {
        let token = self.token(settings).await?;
        let caller = self.caller(&token, settings)?;
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

    /// Logs in as `session`, without calling the hub.
    pub fn login_as(
        &self,
        session: HubSession,
        settings: &Settings,
    ) -> Result<HubLogin, HubCallError> {
        Ok(HubLogin {
            caller: self.caller(&session.token, settings)?,
            user_id: session.user_id,
        })
    }

    fn caller(&self, token: &SecretString, settings: &Settings) -> Result<Caller, HubCallError> {
        let mut authorization =
            MetadataValue::try_from(format!("Bearer {}", token.expose_secret()))
                .map_err(HubCallError::InvalidToken)?;
        authorization.set_sensitive(true);
        Ok(Caller {
            service: self.service.clone(),
            authorization,
            timeout: settings.network_timeout,
        })
    }

    async fn token(&self, settings: &Settings) -> Result<SecretString, HubCallError> {
        // The channel was dialed for `self.hub` and cannot change, so a token for any other hub
        // must not be sent down it. A self-hosted hub's session is only assumed to be the one
        // that logged in at the debug address: the session slot is not keyed by hub.
        let now = settings.hub_endpoint();
        if now != self.hub {
            return Err(HubCallError::HubChanged(now));
        }
        if matches!(self.target, Target::SelfHosted) {
            return Err(HubCallError::SelfHostedHub(now));
        }

        let SyncAuth::Hub { token } = settings.resolve_sync_auth().await else {
            return Err(HubCallError::NotLoggedIn);
        };
        Ok(token)
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

/// The hub session as [`HubClient::session`] read it: its token, and the user it logs in.
#[derive(Debug)]
pub struct HubSession {
    token: SecretString,
    user_id: UserId,
}

impl HubSession {
    #[must_use]
    pub fn user_id(&self) -> &UserId {
        &self.user_id
    }
}

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
        history.record_id = record_id.map(pb::UuidV7::try_from).transpose()?;
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

    /// Stores `batch`'s messages, all or none.
    pub async fn upload_thread_messages(&self, batch: pb::ThreadBatch) -> Result<(), HubCallError> {
        let request = self.caller.request(pb::InsertThreadMessagesRequest::from(batch));

        self.caller
            .service
            .clone()
            .insert_thread_messages(request)
            .await
            .map_err(HubCallError::Hub)?;
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
    use atuin_client::settings::{
        DEFAULT_SYNC_URL, DbLocation, Octavo, OctavoDebug, Settings, SyncProtocol,
    };
    use atuin_common::path::AbsPathBuf;
    use rstest::rstest;
    use url::Url;

    use super::{HubCallError, HubClient};

    const OFFICIAL: &str = "https://api.atuin.sh";
    const SELF_HOSTED: &str = "http://localhost:4000";
    const DEBUG: &str = "http://127.0.0.1:50052";

    fn settings(sync_address: &str, debug_address: Option<&str>) -> Settings {
        let dir = tempfile::tempdir().unwrap().keep();
        let mut settings = Settings {
            sync_address: Url::parse(sync_address).unwrap(),
            sync_protocol: SyncProtocol::Hub,
            data_dir: AbsPathBuf::new(&dir).unwrap(),
            octavo: Octavo {
                debug: OctavoDebug {
                    address: debug_address.map(|url| Url::parse(url).unwrap()),
                },
                ..Octavo::default()
            },
            ..Settings::default()
        };
        settings.meta.db_path = DbLocation::File(AbsPathBuf::new(dir.join("meta.db")).unwrap());
        settings
    }

    #[rstest]
    #[case::official_without_debug(OFFICIAL, None, DEFAULT_SYNC_URL.as_str())]
    #[case::official_ignores_debug(OFFICIAL, Some(DEBUG), DEFAULT_SYNC_URL.as_str())]
    #[case::self_hosted_with_debug(SELF_HOSTED, Some(DEBUG), "http://127.0.0.1:50052/")]
    #[tokio::test]
    async fn dials_the_debug_address_only_for_a_self_hosted_hub(
        #[case] sync_address: &str,
        #[case] debug_address: Option<&str>,
        #[case] expected: &str,
    ) {
        let settings = settings(sync_address, debug_address);

        let client = HubClient::new(&settings).unwrap();

        assert_eq!(client.target.address().as_str(), expected);
    }

    #[rstest]
    #[tokio::test]
    async fn self_hosted_hub_with_debug_address_passes_the_hub_check() {
        let settings = settings(SELF_HOSTED, Some(DEBUG));

        let session = HubClient::new(&settings).unwrap().session(&settings).await;

        assert!(matches!(session, Err(HubCallError::NotLoggedIn)));
    }

    #[rstest]
    #[tokio::test]
    async fn self_hosted_hub_token_stays_home() {
        let settings = settings(SELF_HOSTED, None);

        let session = HubClient::new(&settings).unwrap().session(&settings).await;

        assert!(matches!(session, Err(HubCallError::SelfHostedHub(_))));
    }

    #[rstest]
    #[case::official_to_self_hosted_debug(OFFICIAL, None, SELF_HOSTED, Some(DEBUG))]
    #[case::debug_to_official(SELF_HOSTED, Some(DEBUG), OFFICIAL, None)]
    #[case::debug_to_other_self_hosted(
        SELF_HOSTED,
        Some(DEBUG),
        "http://localhost:5000",
        Some(DEBUG)
    )]
    #[tokio::test]
    async fn a_reload_to_another_hub_is_refused_as_transient(
        #[case] built_sync: &str,
        #[case] built_debug: Option<&str>,
        #[case] reloaded_sync: &str,
        #[case] reloaded_debug: Option<&str>,
    ) {
        let client = HubClient::new(&settings(built_sync, built_debug)).unwrap();
        let reloaded = settings(reloaded_sync, reloaded_debug);

        let error = client.session(&reloaded).await.unwrap_err();

        assert!(matches!(error, HubCallError::HubChanged(_)));
        assert!(!error.is_permanent());
    }

    #[rstest]
    #[tokio::test]
    async fn a_debug_address_added_by_a_reload_does_not_lift_the_self_hosted_refusal() {
        let client = HubClient::new(&settings(SELF_HOSTED, None)).unwrap();
        let reloaded = settings(SELF_HOSTED, Some(DEBUG));

        let session = client.session(&reloaded).await;

        assert!(matches!(session, Err(HubCallError::SelfHostedHub(_))));
    }
}
