use std::time::Duration;

use atuin_client::history::{CommandCapture, HistoryId};
use atuin_client::lua_config::OrgSlug;
use atuin_client::settings::{DEFAULT_HUB_URL, DEFAULT_SYNC_URL, Settings, SyncAuth};
use hyper_tls::HttpsConnector;
use hyper_util::client::legacy::connect::HttpConnector;
use secrecy::{ExposeSecret, SecretString};
use thiserror::Error;
use tonic::metadata::errors::InvalidMetadataValue;
use tonic::metadata::{Ascii, MetadataValue};
use tonic::transport::{Channel, Endpoint};
use tonic::{Code, Request, Status};
use tonic_types::StatusExt as _;
use tracing::warn;
use url::Url;

use crate::pb;
use crate::pb::hub_service_client::HubServiceClient;
use crate::pb::{NotUuidV7, OutputTooLarge};

#[derive(Debug, Error)]
pub enum ConnectError {
    #[error("the Atuin for Teams endpoint {address} is not a valid URI")]
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
    #[error("history {0}")]
    Id(#[from] NotUuidV7<HistoryId>),
    #[error(transparent)]
    OutputTooLarge(#[from] OutputTooLarge),
    #[error("the hub has not stored the history entry yet")]
    HistoryNotStored(#[source] tonic::Status),
    #[error("the hub has not stored the thread yet")]
    ThreadNotStored(#[source] tonic::Status),
    #[error("Atuin for Teams needs a hub login; run `atuin login`")]
    NotLoggedIn,
    #[error("the hub login was saved without its user, which the next upload pass learns")]
    UnknownUser,
    #[error("failed to read the hub login: {0:#}")]
    Meta(eyre::Report),
    #[error(transparent)]
    Connect(#[from] ConnectError),
    #[error("Atuin for Teams runs only on Atuin's hosted hub, not {0}")]
    SelfHostedHub(Url),
    #[error("the hub changed to {0} after Atuin for Teams connected; restart atuin")]
    HubChanged(Url),
    #[error("the hub token is not a valid header value")]
    InvalidToken(#[source] InvalidMetadataValue),
    #[error("the hub named no user for the token")]
    NoUserId,
    #[error("Atuin for Teams is not enabled for this account")]
    OctavoOff(#[source] Status),
    #[error("the hub refused the call")]
    Hub(#[source] Status),
}

impl From<Status> for HubCallError {
    fn from(status: Status) -> Self {
        /// The `google.rpc.ErrorInfo` reason of a call refused as its account has Octavo off.
        const OCTAVO_NOT_ENABLED: &str = "OCTAVO_NOT_ENABLED";

        let off = status.code() == Code::FailedPrecondition
            && status
                .get_details_error_info()
                .is_some_and(|info| info.reason == OCTAVO_NOT_ENABLED);
        if off {
            return Self::OctavoOff(status);
        }
        Self::Hub(status)
    }
}

impl HubCallError {
    /// Whether the call would fail the same way however often it's retried.
    #[must_use]
    pub fn is_permanent(&self) -> bool {
        match self {
            Self::Id(_) | Self::OutputTooLarge(_) => true,
            Self::Hub(status) => status.code() == Code::InvalidArgument,
            Self::NotLoggedIn
            | Self::UnknownUser
            | Self::Meta(_)
            | Self::Connect(_)
            | Self::SelfHostedHub(_)
            | Self::HubChanged(_)
            | Self::InvalidToken(_)
            | Self::HistoryNotStored(_)
            | Self::ThreadNotStored(_)
            | Self::NoUserId
            // Until the account has Octavo again.
            | Self::OctavoOff(_) => false,
        }
    }
}

/// A hub user's id, as `WhoAmI` names it. Opaque.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct UserId(String);

impl UserId {
    #[must_use]
    pub fn new(id: &str) -> Self {
        Self(id.to_owned())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Where a [`HubClient`] uploads, decided once when it is built.
#[derive(Debug, Clone)]
enum Target {
    /// Atuin's hosted hub.
    Official,
    /// A self-hosted hub, which Octavo refuses: there is nowhere to upload to.
    SelfHosted,
    /// The `teams.__debug__.address` of a self-hosted hub.
    Debug(Url),
}

impl Target {
    fn of(settings: &Settings) -> Self {
        if settings.hub_endpoint() == *DEFAULT_HUB_URL {
            return Self::Official;
        }
        match &settings.teams.debug.address {
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
        if settings.teams.debug.address.is_some() && matches!(target, Target::Official) {
            warn!(
                "teams.__debug__.address is honored only while sync_address is a self-hosted hub \
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

    pub async fn upload_history(&self, batch: pb::HistoryBatch) -> Result<(), HubCallError> {
        let request = self.caller.request(pb::InsertHistoryRequest::from(batch));

        self.caller.service.clone().insert_history(request).await.map_err(HubCallError::from)?;
        Ok(())
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
            Err(status) => Err(status.into()),
        }
    }

    /// Deletes the entry with `id`, and keeps it from being stored again.
    pub async fn delete(&self, id: HistoryId) -> Result<(), HubCallError> {
        let request = self.caller.request(pb::DeleteHistoryRequest {
            id: Some(id.try_into()?),
        });

        self.caller.service.clone().delete_history(request).await.map_err(HubCallError::from)?;
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
            .map_err(HubCallError::from)?;
        Ok(())
    }

    /// Opens `thread` to `orgs`, returning the slugs of those the user belongs to none with, which
    /// it leaves out.
    ///
    /// Fails with [`HubCallError::ThreadNotStored`] until the thread is stored.
    pub(crate) async fn share_thread_with_orgs(
        &self,
        thread: &pb::HarnessSession,
        orgs: &[OrgSlug],
    ) -> Result<Vec<String>, HubCallError> {
        let request = self.caller.request(pb::ShareThreadWithOrgsRequest {
            thread: Some(thread.clone()),
            org_slugs: orgs.iter().map(ToString::to_string).collect(),
        });

        match self.caller.service.clone().share_thread_with_orgs(request).await {
            Ok(response) => Ok(response.into_inner().skipped_org_slugs),
            Err(status) if status.code() == Code::NotFound => {
                Err(HubCallError::ThreadNotStored(status))
            }
            Err(status) => Err(status.into()),
        }
    }

    /// Closes `thread` to those of `orgs` [`Self::share_thread_with_orgs`] opened it to, which it
    /// never opens it to again.
    ///
    /// Fails with [`HubCallError::ThreadNotStored`] until the thread is stored.
    pub(crate) async fn close_thread_to_orgs(
        &self,
        thread: &pb::HarnessSession,
        orgs: &[OrgSlug],
    ) -> Result<(), HubCallError> {
        let request = self.caller.request(pb::CloseThreadToOrgsRequest {
            thread: Some(thread.clone()),
            org_slugs: orgs.iter().map(ToString::to_string).collect(),
        });

        match self.caller.service.clone().close_thread_to_orgs(request).await {
            Ok(_response) => Ok(()),
            Err(status) if status.code() == Code::NotFound => {
                Err(HubCallError::ThreadNotStored(status))
            }
            Err(status) => Err(status.into()),
        }
    }
}

#[cfg(test)]
impl HubLogin {
    /// A login as `user` to a hub nobody answers at, for a test that never calls it.
    pub(crate) fn offline(user: &str) -> Self {
        let channel = Endpoint::from_static("http://127.0.0.1:9").connect_lazy();
        Self {
            caller: Caller {
                service: HubServiceClient::new(channel),
                authorization: MetadataValue::from_static("Bearer offline"),
                timeout: Duration::from_secs(1),
            },
            user_id: UserId::new(user),
        }
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
            self.service.clone().who_am_i(request).await.map_err(HubCallError::from)?.into_inner();

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
        DEFAULT_SYNC_URL, DbLocation, Settings, SyncProtocol, Teams, TeamsDebug,
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
            teams: Teams {
                debug: TeamsDebug {
                    address: debug_address.map(|url| Url::parse(url).unwrap()),
                    mcp_address: None,
                },
                ..Teams::default()
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
