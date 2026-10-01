//! Generated operations end to end against a server mounted under a path prefix, as a self-hosted
//! server can be.

use atuin_api_client::{ApiError, Client, MapApiError, Secret, types};
use atuin_domain::record::{
    EncryptedData, Host, HostId, Record, RecordId, RecordTag, RecordVersion,
};
use pretty_assertions::assert_eq;
use reqwest::StatusCode;
use rstest::{fixture, rstest};
use secrecy::ExposeSecret;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use url::Url;
use uuid::Uuid;
use wiremock::matchers::{body_json, method, path, query_param};
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

    async fn only_request(&self) -> wiremock::Request {
        let mut requests = self.mock.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1, "expected exactly one request: {requests:?}");
        requests.remove(0)
    }
}

#[fixture]
async fn server() -> Server {
    let mock = MockServer::start().await;
    let base = Url::parse(&format!("{}{PREFIX}/", mock.uri())).unwrap();
    let client = Client::from_http(&base, reqwest::Client::new()).unwrap();
    Server { mock, client }
}

fn record() -> Record<EncryptedData> {
    Record::builder()
        .id(RecordId(Uuid::from_u128(1)))
        .idx(7)
        .host(Host::new(HostId(Uuid::from_u128(2))))
        .timestamp(1_700_000_000_000_000_000)
        .version(RecordVersion::V0)
        .tag(RecordTag::History)
        .data(EncryptedData {
            raw: "v4.local.payload".into(),
            cek: "wrapped".into(),
        })
        .build()
}

/// [`record`] as the API carries it.
fn record_json() -> Value {
    json!({
        "id": "00000000-0000-0000-0000-000000000001",
        "idx": 7,
        "host": {"id": "00000000-0000-0000-0000-000000000002", "name": ""},
        "timestamp": 1_700_000_000_000_000_000_u64,
        "version": "v0",
        "tag": "history",
        "data": {"data": "v4.local.payload", "content_encryption_key": "wrapped"},
    })
}

#[rstest]
#[tokio::test]
async fn get_me_decodes_the_body_and_keeps_status_and_headers(#[future] server: Server) {
    let server = server.await;
    server
        .mount(
            Mock::given(method("GET")).and(path("/atuin/api/v0/me")).respond_with(
                ResponseTemplate::new(200)
                    .insert_header("atuin-version", "18.23.0")
                    .set_body_json(json!({"username": "ellie"})),
            ),
        )
        .await;

    let me = server.client.get_me().map_api_error().await.unwrap();

    assert_eq!(me.status(), StatusCode::OK);
    assert_eq!(me.headers()["atuin-version"], "18.23.0");
    assert_eq!(me.into_inner().username.as_deref(), Some("ellie"));
}

#[rstest]
#[tokio::test]
async fn api_version_is_never_sent(#[future] server: Server) {
    let server = server.await;
    server
        .mount(Mock::given(method("GET")).and(path("/atuin/")).respond_with(
            ResponseTemplate::new(200).set_body_json(json!({
                "homage": "Through the fire and flames",
                "version": "18.23.0",
            })),
        ))
        .await;

    server.client.get_index().map_api_error().await.unwrap();

    let request = server.only_request().await;
    assert!(!request.headers.contains_key("api-version"), "{:?}", request.headers);
}

#[rstest]
#[tokio::test]
async fn login_sends_secrets_in_plaintext_and_decodes_the_session(#[future] server: Server) {
    let server = server.await;
    server
        .mount(
            Mock::given(method("POST"))
                .and(path("/atuin/api/v0/login"))
                .and(body_json(json!({"username": "ellie", "password": "hunter2"})))
                .respond_with(
                    ResponseTemplate::new(200)
                        .set_body_json(json!({"session": "s3ss10n", "auth": "cli"})),
                ),
        )
        .await;
    let body = types::LoginRequest {
        username: "ellie".into(),
        password: Secret::from("hunter2"),
        totp_code: None,
    };

    let login = server.client.login(&body).map_api_error().await.unwrap().into_inner();

    assert_eq!(login.session.expose_secret(), "s3ss10n");
    assert_eq!(login.auth.as_deref(), Some("cli"));
}

#[rstest]
#[tokio::test]
async fn verify_sends_the_code_as_a_query_parameter(#[future] server: Server) {
    let server = server.await;
    server
        .mount(
            Mock::given(method("POST"))
                .and(path("/atuin/auth/cli/verify"))
                .and(query_param("code", "hunter2"))
                .respond_with(
                    ResponseTemplate::new(200)
                        .set_body_json(json!({"success": true, "token": "atapi_token"})),
                ),
        )
        .await;

    let verified = server
        .client
        .verify_cli_auth_code(&Secret::from("hunter2"))
        .map_api_error()
        .await
        .unwrap()
        .into_inner();

    assert_eq!(verified.token.expose_secret(), "atapi_token");
}

#[rstest]
#[tokio::test]
async fn delete_account_sends_its_body_on_a_delete(#[future] server: Server) {
    let server = server.await;
    server
        .mount(
            Mock::given(method("DELETE"))
                .and(path("/atuin/api/v0/account"))
                .and(body_json(json!({"password": "hunter2", "totp_code": "123456"})))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({}))),
        )
        .await;
    let body = types::DeleteUserRequest {
        password: Secret::from("hunter2"),
        totp_code: Some(Secret::from("123456")),
    };

    server.client.delete_account(&body).map_api_error().await.unwrap();
}

#[rstest]
#[tokio::test]
async fn post_records_sends_a_bare_array_of_domain_records(#[future] server: Server) {
    let server = server.await;
    server
        .mount(
            Mock::given(method("POST"))
                .and(path("/atuin/api/v0/record"))
                .and(body_json(json!([record_json()])))
                .respond_with(ResponseTemplate::new(200)),
        )
        .await;

    server.client.post_records(&vec![record()]).map_api_error().await.unwrap();
}

#[rstest]
#[tokio::test]
async fn get_next_records_sends_its_query_and_decodes_domain_records(#[future] server: Server) {
    let server = server.await;
    server
        .mount(
            Mock::given(method("GET"))
                .and(path("/atuin/api/v0/record/next"))
                .and(query_param("host", "00000000-0000-0000-0000-000000000002"))
                .and(query_param("tag", "history"))
                .and(query_param("start", "7"))
                .and(query_param("count", "100"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!([record_json()]))),
        )
        .await;

    let records = server
        .client
        .get_next_records(&100, &Uuid::from_u128(2), Some(&7), "history")
        .map_api_error()
        .await
        .unwrap()
        .into_inner();

    assert_eq!(records, vec![record()]);
}

#[rstest]
#[tokio::test]
async fn path_parameters_are_one_encoded_segment(#[future] server: Server) {
    let server = server.await;
    server
        .mount(Mock::given(method("GET")).and(path("/atuin/user/john%20doe%2Fadmin")).respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"username": "john doe/admin"})),
        ))
        .await;

    let user = server.client.legacy_get_user("john doe/admin").map_api_error().await.unwrap();

    assert_eq!(user.into_inner().username, "john doe/admin");
}

#[rstest]
#[case::reason_and_code(
    ResponseTemplate::new(403).set_body_json(json!({"reason": "2FA required", "code": "2fa_required"})),
    StatusCode::FORBIDDEN,
    (Some("2FA required"), Some("2fa_required"), None)
)]
#[case::legacy_errors(
    ResponseTemplate::new(401).set_body_json(json!({"errors": ["Unauthorized"]})),
    StatusCode::UNAUTHORIZED,
    (Some("Unauthorized"), None, None)
)]
#[case::ai_envelope(
    ResponseTemplate::new(500).set_body_json(json!({"error": "internal_error", "message": "boom"})),
    StatusCode::INTERNAL_SERVER_ERROR,
    (Some("boom"), None, None)
)]
#[case::plain_text(
    ResponseTemplate::new(412).set_body_string("capabilities out of date"),
    StatusCode::PRECONDITION_FAILED,
    (None, None, Some("capabilities out of date"))
)]
#[case::empty(ResponseTemplate::new(503), StatusCode::SERVICE_UNAVAILABLE, (None, None, None))]
#[tokio::test]
async fn errors_keep_the_status_and_parse_any_body(
    #[future] server: Server,
    #[case] response: ResponseTemplate,
    #[case] status: StatusCode,
    #[case] said: (Option<&str>, Option<&str>, Option<&str>),
) {
    let server = server.await;
    server
        .mount(Mock::given(method("GET")).and(path("/atuin/api/v0/me")).respond_with(response))
        .await;

    let err = server.client.get_me().map_api_error().await.unwrap_err();

    let ApiError::Status {
        status: found_status,
        url,
        reason,
        code,
        body,
    } = err
    else {
        panic!("a non-2xx answer is a Status error: {err:?}");
    };
    assert_eq!(
        (found_status, url.path(), (reason.as_deref(), code.as_deref(), body.as_deref())),
        (status, "/atuin/api/v0/me", said)
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
        .mount(Mock::given(method("POST")).and(path("/atuin/auth/cli/verify")).respond_with(
            ResponseTemplate::new(404).set_body_json(json!({"reason": "unknown code"})),
        ))
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
