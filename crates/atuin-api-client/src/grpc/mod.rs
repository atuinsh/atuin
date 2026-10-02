mod error;
pub mod pb;

use std::time::Duration;

pub use error::{CallError, HistoryStreamError, InsertHistoryError, NewHubClientError};
use hyper_tls::HttpsConnector;
use hyper_util::client::legacy::connect::HttpConnector;
use pb::hub_service_client::HubServiceClient;
use secrecy::{ExposeSecret, SecretString};
use tokio_stream::{Stream, StreamExt};
use tonic::metadata::errors::InvalidMetadataValue;
use tonic::metadata::{Ascii, MetadataValue};
use tonic::service::Interceptor;
use tonic::service::interceptor::InterceptedService;
use tonic::transport::{Channel, Endpoint};
use tonic::{Request, Status};
use url::Url;

const USER_AGENT: &str = concat!("atuin/", env!("CARGO_PKG_VERSION"));

#[derive(Debug)]
pub struct HubConfig {
    pub endpoint: Url,
    pub token: SecretString,
    pub connect_timeout: Duration,
    pub unary_timeout: Duration,
}

#[derive(Debug, Clone)]
pub struct HubClient {
    inner: HubServiceClient<InterceptedService<Channel, BearerToken>>,
    unary_timeout: Duration,
}

impl HubClient {
    pub fn new(config: &HubConfig) -> Result<Self, NewHubClientError> {
        let scheme = config.endpoint.scheme();
        if !matches!(scheme, "http" | "https") {
            return Err(NewHubClientError::UnsupportedScheme(scheme.to_owned()));
        }
        let token = BearerToken::new(&config.token).map_err(NewHubClientError::InvalidToken)?;
        let connector = connector().map_err(NewHubClientError::Tls)?;
        let channel = Endpoint::from_shared(config.endpoint.to_string())
            .map_err(NewHubClientError::InvalidEndpoint)?
            .user_agent(USER_AGENT)
            .expect("the user agent is a valid header value")
            .connect_timeout(config.connect_timeout)
            .connect_with_connector_lazy(connector);
        Ok(Self {
            inner: HubServiceClient::with_interceptor(channel, token),
            unary_timeout: config.unary_timeout,
        })
    }

    pub async fn insert_history(&self, history: pb::History) -> Result<(), InsertHistoryError> {
        let mut request = Request::new(pb::InsertHistoryRequest {
            history: Some(history),
        });
        request.set_timeout(self.unary_timeout);
        self.inner.clone().insert_history(request).await?;
        Ok(())
    }

    pub async fn watch_history(
        &self,
    ) -> Result<impl Stream<Item = Result<pb::History, HistoryStreamError>> + use<>, CallError>
    {
        let responses =
            self.inner.clone().watch_history(pb::WatchHistoryRequest {}).await?.into_inner();
        Ok(responses.map(watched_history))
    }
}

fn connector() -> Result<HttpsConnector<HttpConnector>, native_tls::Error> {
    let mut http = HttpConnector::new();
    http.enforce_http(false);
    http.set_nodelay(true);
    let tls = native_tls::TlsConnector::builder().request_alpns(&["h2"]).build()?;
    Ok(HttpsConnector::from((http, tls.into())))
}

fn watched_history(
    response: Result<pb::WatchHistoryResponse, Status>,
) -> Result<pb::History, HistoryStreamError> {
    response.map_err(CallError::from)?.history.ok_or(HistoryStreamError::MissingHistory)
}

#[derive(Debug, Clone)]
struct BearerToken(MetadataValue<Ascii>);

impl BearerToken {
    fn new(token: &SecretString) -> Result<Self, InvalidMetadataValue> {
        let mut value = MetadataValue::try_from(format!("Bearer {}", token.expose_secret()))?;
        value.set_sensitive(true);
        Ok(Self(value))
    }
}

impl Interceptor for BearerToken {
    fn call(&mut self, mut request: Request<()>) -> Result<Request<()>, Status> {
        request.metadata_mut().insert("authorization", self.0.clone());
        Ok(request)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::time::Duration;

    use rstest::{fixture, rstest};
    use secrecy::SecretString;
    use tokio::net::TcpListener;
    use tokio::sync::Mutex;
    use tokio_stream::StreamExt;
    use tokio_stream::wrappers::TcpListenerStream;
    use tonic::transport::Server;
    use tonic::{Code, Request, Response, Status};
    use tonic_types::{ErrorDetails, StatusExt};
    use url::Url;

    use super::pb::hub_service_server::{HubService, HubServiceServer};
    use super::{
        CallError, HistoryStreamError, HubClient, HubConfig, InsertHistoryError, NewHubClientError,
        pb,
    };

    const TOKEN: &str = "hub-token";

    #[derive(Default)]
    struct FakeHub {
        refusal: Option<fn() -> Status>,
        watched: Vec<pb::WatchHistoryResponse>,
        inserted: Mutex<Vec<(Option<String>, pb::InsertHistoryRequest)>>,
    }

    #[tonic::async_trait]
    impl HubService for FakeHub {
        type WatchHistoryStream =
            tokio_stream::Iter<std::vec::IntoIter<Result<pb::WatchHistoryResponse, Status>>>;

        async fn insert_history(
            &self,
            request: Request<pb::InsertHistoryRequest>,
        ) -> Result<Response<pb::InsertHistoryResponse>, Status> {
            let authorization = request
                .metadata()
                .get("authorization")
                .map(|value| value.to_str().unwrap().to_owned());
            self.inserted.lock().await.push((authorization, request.into_inner()));
            self.refusal.map_or_else(
                || Ok(Response::new(pb::InsertHistoryResponse {})),
                |refusal| Err(refusal()),
            )
        }

        async fn watch_history(
            &self,
            _request: Request<pb::WatchHistoryRequest>,
        ) -> Result<Response<Self::WatchHistoryStream>, Status> {
            if let Some(refusal) = self.refusal {
                return Err(refusal());
            }
            let responses: Vec<_> = self.watched.iter().cloned().map(Ok).collect();
            Ok(Response::new(tokio_stream::iter(responses)))
        }
    }

    async fn serve(hub: Arc<FakeHub>) -> Url {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = Url::parse(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
        tokio::spawn(
            Server::builder()
                .add_service(HubServiceServer::from_arc(hub))
                .serve_with_incoming(TcpListenerStream::new(listener)),
        );
        endpoint
    }

    fn config(endpoint: Url) -> HubConfig {
        HubConfig {
            endpoint,
            token: SecretString::from(TOKEN),
            connect_timeout: Duration::from_secs(5),
            unary_timeout: Duration::from_secs(5),
        }
    }

    fn client(endpoint: Url) -> HubClient {
        HubClient::new(&config(endpoint)).unwrap()
    }

    fn history(command: &str) -> pb::History {
        pb::History {
            id: Some(pb::UuidV7 {
                value: command.bytes().cycle().take(16).collect(),
            }),
            command: command.to_owned(),
            ..pb::History::default()
        }
    }

    fn hub_error(code: Code, reason: &str) -> Status {
        foreign_error(code, reason, "hub.atuin.sh")
    }

    fn foreign_error(code: Code, reason: &str, domain: &str) -> Status {
        Status::with_error_details(
            code,
            "refused",
            ErrorDetails::with_error_info(reason, domain, HashMap::new()),
        )
    }

    #[fixture]
    fn unreachable() -> Url {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        Url::parse(&format!("http://{}", listener.local_addr().unwrap())).unwrap()
    }

    #[rstest]
    #[tokio::test]
    async fn insert_sends_the_token_and_history() {
        let hub = Arc::new(FakeHub::default());
        let client = client(serve(Arc::clone(&hub)).await);

        client.insert_history(history("ls")).await.unwrap();

        let inserted = hub.inserted.lock().await;
        assert_eq!(*inserted, [(Some(format!("Bearer {TOKEN}")), pb::InsertHistoryRequest {
            history: Some(history("ls"))
        })]);
    }

    #[rstest]
    #[case::invalid_history(
        || hub_error(Code::InvalidArgument, "INVALID_HISTORY"),
        |err: &_| matches!(err, InsertHistoryError::Invalid(_)),
    )]
    #[case::already_exists(
        || hub_error(Code::AlreadyExists, "HISTORY_ALREADY_EXISTS"),
        |err: &_| matches!(err, InsertHistoryError::AlreadyExists(_)),
    )]
    #[case::not_enabled(
        || hub_error(Code::FailedPrecondition, "OCTAVO_NOT_ENABLED"),
        |err: &_| matches!(err, InsertHistoryError::Call(CallError::NotEnabled(_))),
    )]
    #[case::invalid_token(
        || hub_error(Code::Unauthenticated, "INVALID_TOKEN"),
        |err: &_| matches!(err, InsertHistoryError::Call(CallError::Unauthenticated(_))),
    )]
    #[case::unavailable(
        || hub_error(Code::Unavailable, "UNAVAILABLE"),
        |err: &_| matches!(err, InsertHistoryError::Call(CallError::Unavailable(_))),
    )]
    #[case::malformed_request(
        || hub_error(Code::InvalidArgument, "MALFORMED_REQUEST"),
        |err: &_| matches!(err, InsertHistoryError::Call(CallError::Other(_))),
    )]
    #[case::internal(
        || hub_error(Code::Internal, "INTERNAL"),
        |err: &_| matches!(err, InsertHistoryError::Call(CallError::Other(_))),
    )]
    #[case::reason_from_another_domain(
        || foreign_error(Code::AlreadyExists, "HISTORY_ALREADY_EXISTS", "example.com"),
        |err: &_| matches!(err, InsertHistoryError::Call(CallError::Other(_))),
    )]
    #[case::no_error_info(
        || Status::already_exists("refused"),
        |err: &_| matches!(err, InsertHistoryError::Call(CallError::Other(_))),
    )]
    #[tokio::test]
    async fn insert_maps_hub_errors(
        #[case] refusal: fn() -> Status,
        #[case] expected: fn(&InsertHistoryError) -> bool,
    ) {
        let hub = Arc::new(FakeHub {
            refusal: Some(refusal),
            ..FakeHub::default()
        });
        let client = client(serve(hub).await);

        let err = client.insert_history(history("ls")).await.unwrap_err();

        assert!(expected(&err), "{err:?}");
    }

    #[rstest]
    #[tokio::test]
    async fn calls_to_an_unreachable_hub_are_unavailable(unreachable: Url) {
        let err = client(unreachable).insert_history(history("ls")).await.unwrap_err();

        assert!(matches!(err, InsertHistoryError::Call(CallError::Unavailable(_))), "{err:?}");
    }

    #[rstest]
    #[tokio::test]
    async fn watch_yields_entries_in_order() {
        let entries = [history("ls"), history("cd"), history("pwd")];
        let hub = Arc::new(FakeHub {
            watched: entries
                .iter()
                .cloned()
                .map(|history| pb::WatchHistoryResponse {
                    history: Some(history),
                })
                .collect(),
            ..FakeHub::default()
        });
        let client = client(serve(hub).await);

        let watched: Vec<_> =
            client.watch_history().await.unwrap().collect::<Result<_, _>>().await.unwrap();

        assert_eq!(watched, entries);
    }

    #[rstest]
    #[tokio::test]
    async fn watch_fails_on_a_response_without_history() {
        let hub = Arc::new(FakeHub {
            watched: vec![pb::WatchHistoryResponse { history: None }],
            ..FakeHub::default()
        });
        let client = client(serve(hub).await);

        let mut watched = client.watch_history().await.unwrap();
        let err = watched.next().await.unwrap().unwrap_err();

        assert!(matches!(err, HistoryStreamError::MissingHistory), "{err:?}");
    }

    #[rstest]
    #[tokio::test]
    async fn watch_maps_a_refused_call() {
        let hub = Arc::new(FakeHub {
            refusal: Some(|| hub_error(Code::FailedPrecondition, "OCTAVO_NOT_ENABLED")),
            ..FakeHub::default()
        });
        let client = client(serve(hub).await);

        let Err(err) = client.watch_history().await else {
            panic!("the refused watch must fail");
        };

        assert!(matches!(err, CallError::NotEnabled(_)), "{err:?}");
    }

    #[rstest]
    #[case::unsupported_scheme("ftp://hub.atuin.sh", TOKEN, |err: &_| matches!(err, NewHubClientError::UnsupportedScheme(scheme) if scheme == "ftp"))]
    #[case::token_with_a_newline("https://hub.atuin.sh", "hub\ntoken", |err: &_| matches!(err, NewHubClientError::InvalidToken(_)))]
    fn new_rejects_a_bad_config(
        #[case] endpoint: &str,
        #[case] token: &str,
        #[case] expected: fn(&NewHubClientError) -> bool,
    ) {
        let config = HubConfig {
            token: SecretString::from(token),
            ..config(Url::parse(endpoint).unwrap())
        };

        let err = HubClient::new(&config).unwrap_err();

        assert!(expected(&err), "{err:?}");
    }

    #[rstest]
    #[tokio::test]
    async fn debug_output_hides_the_token(unreachable: Url) {
        let client = client(unreachable);

        assert!(!format!("{client:?}").contains(TOKEN), "{client:?}");
    }
}
