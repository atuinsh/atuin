use std::env;
use std::time::Duration;

use atuin_api_client::{Client, MapApiError, types};
use atuin_common::utils::uuid_v7;
use atuin_server::db::DbSettings;
use atuin_server::{Settings as ServerSettings, launch_with_tcp_listener};
use futures_util::TryFutureExt;
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue};
use secrecy::{ExposeSecret, SecretString};
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tracing::{Dispatch, dispatcher};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt;

pub async fn start_server(path: &str) -> (url::Url, oneshot::Sender<()>, JoinHandle<()>) {
    let formatting_layer = tracing_tree::HierarchicalLayer::default()
        .with_writer(tracing_subscriber::fmt::TestWriter::new())
        .with_indent_lines(true)
        .with_ansi(true)
        .with_targets(true)
        .with_indent_amount(2);

    let dispatch: Dispatch = tracing_subscriber::registry()
        .with(formatting_layer)
        .with(EnvFilter::new("atuin_server=debug,atuin_client=debug,info"))
        .into();

    let db_uri = env::var("ATUIN_DB_URI")
        .unwrap_or_else(|_| "postgres://atuin:pass@localhost:5432/atuin".to_owned());

    let server_settings = ServerSettings {
        host: "127.0.0.1".to_owned(),
        port: 0,
        path: path.to_owned(),
        open_registration: true,
        max_record_size: atuin_common::units::ByteSize::b(1024 * 1024 * 1024),
        register_webhook_url: None,
        register_webhook_username: String::new(),
        db_settings: DbSettings {
            db_uri: db_uri.parse().expect("invalid ATUIN_DB_URI"),
        },
        metrics: atuin_server::settings::Metrics::default(),
        fake_version: None,
    };

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let _tracing_guard = dispatcher::set_default(&dispatch);

        if let Err(e) =
            launch_with_tcp_listener(server_settings, listener, shutdown_rx.unwrap_or_else(|_| ()))
                .await
        {
            tracing::error!(error=?e, "server error");
            panic!("error running server: {e:?}");
        }
    });

    // let the server come online
    tokio::time::sleep(Duration::from_millis(200)).await;

    let url = url::Url::parse(&format!("http://{addr}{path}"))
        .expect("test server address is a valid URL");

    (url, shutdown_tx, server)
}

/// A client for the server at `address`, authenticated with the CLI session `session`, if any.
pub fn client(address: &url::Url, session: Option<&SecretString>) -> Client {
    let mut headers = HeaderMap::new();
    if let Some(session) = session {
        let mut value =
            HeaderValue::from_str(&format!("Token {}", session.expose_secret())).unwrap();
        value.set_sensitive(true);
        headers.insert(AUTHORIZATION, value);
    }
    let http = reqwest::Client::builder().default_headers(headers).build().unwrap();
    Client::from_http(address.clone(), http).unwrap()
}

/// Register `username`, and return a client authenticated as them.
pub async fn register_inner(address: &url::Url, username: &str, password: &str) -> Client {
    let body = types::RegisterRequest {
        email: format!("{}@example.com", uuid_v7().as_simple()),
        username: username.to_owned(),
        password: password.into(),
    };
    let registered = client(address, None).legacy_register(&body).map_api_error().await.unwrap();

    assert!(registered.headers().contains_key("atuin-version"), "{:?}", registered.headers());
    client(address, Some(&registered.into_inner().session.into()))
}

#[allow(dead_code)]
pub async fn login(address: &url::Url, username: String, password: String) -> Client {
    let body = types::LoginRequest {
        username,
        password: password.into(),
        totp_code: None,
    };
    let session =
        client(address, None).legacy_login(&body).map_api_error().await.unwrap().into_inner();

    client(address, Some(&session.session.into()))
}

#[allow(dead_code)]
pub async fn register(address: &url::Url) -> Client {
    let username = uuid_v7().as_simple().to_string();
    let password = uuid_v7().as_simple().to_string();
    register_inner(address, &username, &password).await
}

/// The username `client` is authenticated as.
pub async fn username(client: &Client) -> Option<String> {
    client.get_me().map_api_error().await.unwrap().into_inner().username
}
