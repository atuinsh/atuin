//! Generated operations end to end against a server mounted under a path prefix, as a self-hosted
//! server can be.

use atuin_api_client::{ApiError, Client, MapApiError, Secret, types};
use pretty_assertions::assert_eq;
use reqwest::StatusCode;
use rstest::{fixture, rstest};
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use url::Url;
use uuid::Uuid;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const PREFIX: &str = "/atuin";

/// A mock server, and a client whose base URL is its prefix with a trailing slash.
struct Server {
    mock: MockServer,
    client: Client,
}

impl Server {
    async fn mount(&self, mock: Mock) {
        mock.mount(&self.mock).await;
    }
}

#[fixture]
async fn server() -> Server {
    let mock = MockServer::start().await;
    let base = Url::parse(&format!("{}{PREFIX}/", mock.uri())).unwrap();
    let client = Client::from_http(&base, reqwest::Client::new()).unwrap();
    Server { mock, client }
}

#[rstest]
#[tokio::test]
async fn errors_keep_the_status_and_parse_the_body(#[future] server: Server) {
    let server = server.await;
    server
        .mount(
            Mock::given(method("GET")).and(path("/atuin/api/v0/me")).respond_with(
                ResponseTemplate::new(403)
                    .set_body_json(json!({"reason": "2FA required", "code": "2fa_required"})),
            ),
        )
        .await;

    let err = server.client.get_me().map_api_error().await.unwrap_err();

    let ApiError::Status {
        status,
        url,
        reason,
        code,
        body,
    } = err
    else {
        panic!("a non-2xx answer is a Status error: {err:?}");
    };
    assert_eq!(
        (status, url.path(), (reason.as_deref(), code.as_deref(), body.as_deref())),
        (
            StatusCode::FORBIDDEN,
            "/atuin/api/v0/me",
            (Some("2FA required"), Some("2fa_required"), None)
        )
    );
}

/// A base URL's query reaches the server ahead of the operation's own, as `append_path` kept it,
/// and its fragment never does.
#[rstest]
#[tokio::test]
async fn the_base_query_leads_every_request() {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/atuin/api/v0/me"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"username": "ellie"})))
        .mount(&mock)
        .await;
    Mock::given(method("GET"))
        .and(path("/atuin/api/v0/record/next"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&mock)
        .await;
    let base = Url::parse(&format!("{}{PREFIX}/?tok=1&b=%20#top", mock.uri())).unwrap();
    let client = Client::from_http(&base, reqwest::Client::new()).unwrap();

    client.get_me().map_api_error().await.unwrap();
    client
        .get_next_records(&2, &Uuid::from_u128(2), Some(&0), "history")
        .map_api_error()
        .await
        .unwrap();

    let requests = mock.received_requests().await.unwrap();
    let sent: Vec<_> = requests
        .iter()
        .map(|request| (request.url.path(), request.url.query(), request.url.fragment()))
        .collect();
    assert_eq!(sent, [
        ("/atuin/api/v0/me", Some("tok=1&b=%20"), None),
        (
            "/atuin/api/v0/record/next",
            Some(
                "tok=1&b=%20&count=2&host=00000000-0000-0000-0000-000000000002&start=0&tag=history"
            ),
            None
        ),
    ]);
}

/// The status is the answer: a body that breaks off loses only its detail.
#[rstest]
#[tokio::test]
async fn an_error_whose_body_breaks_off_keeps_its_status() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let _ = socket.read(&mut [0; 4096]).await;
        socket
            .write_all(b"HTTP/1.1 500 Internal Server Error\r\ncontent-length: 1000\r\n\r\npartial")
            .await
            .unwrap();
    });
    let client = Client::from_http(&base, reqwest::Client::new()).unwrap();

    let err = client.get_me().map_api_error().await.unwrap_err();

    assert!(
        matches!(err, ApiError::Status {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            reason: None,
            body: None,
            ..
        }),
        "{err:?}"
    );
}

#[rstest]
#[tokio::test]
async fn an_error_on_the_verify_route_never_prints_the_code(#[future] server: Server) {
    let server = server.await;
    server
        .mount(
            Mock::given(method("POST"))
                .and(path("/atuin/auth/cli/verify"))
                .and(query_param("code", "hunter2"))
                .respond_with(
                    ResponseTemplate::new(404).set_body_json(json!({"reason": "unknown code"})),
                )
                .expect(1),
        )
        .await;

    let err = server
        .client
        .verify_cli_auth_code(&Secret::from("hunter2"))
        .map_api_error()
        .await
        .unwrap_err();

    assert_eq!(err.status(), Some(StatusCode::NOT_FOUND));
    let printed = format!("{err} {err:?}");
    assert!(!printed.contains("hunter2"), "{printed}");
}

#[rstest]
#[tokio::test]
async fn a_transport_error_never_prints_the_code() {
    // Port 1 (tcpmux) is privileged and unserved, so the connection is refused.
    let base = Url::parse("http://127.0.0.1:1/atuin").unwrap();
    let client = Client::from_http(&base, reqwest::Client::new()).unwrap();

    let err =
        client.verify_cli_auth_code(&Secret::from("hunter2")).map_api_error().await.unwrap_err();

    assert!(matches!(err, ApiError::Transport(_)), "{err:?}");
    let printed = format!("{err} {err:?}");
    assert!(!printed.contains("hunter2"), "{printed}");
}

#[rstest]
#[tokio::test]
async fn a_2xx_body_off_the_api_is_a_decode_error_without_the_body(#[future] server: Server) {
    let server = server.await;
    server
        .mount(
            Mock::given(method("POST")).and(path("/atuin/api/v0/login")).respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"session": "s3ss10n", "auth": ["not", "a", "string"]})),
            ),
        )
        .await;
    let body = types::LoginRequest {
        username: "ellie".into(),
        password: Secret::from("hunter2"),
        totp_code: None,
    };

    let err = server.client.login(&body).map_api_error().await.unwrap_err();

    assert!(matches!(err, ApiError::Decode(_)), "{err:?}");
    let printed = format!("{err} {err:?}");
    assert!(!printed.contains("s3ss10n"), "{printed}");
}
