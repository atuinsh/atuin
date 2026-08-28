use std::env;
use std::time::Duration;

use atuin_client::api_client;
use atuin_common::utils::uuid_v7;
use atuin_server::db::DbSettings;
use atuin_server::{Settings as ServerSettings, launch_with_tcp_listener};
use futures_util::TryFutureExt;
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
        max_record_size: 1024 * 1024 * 1024,
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

pub async fn register_inner(
    address: &url::Url,
    username: &str,
    password: &str,
) -> api_client::Client {
    let email = format!("{}@example.com", uuid_v7().as_simple());

    // registration works
    let registration_response =
        api_client::register(address, username, &email, password, &Default::default())
            .await
            .unwrap();

    let caps = api_client::caps_client_anonymous(address, &Default::default()).unwrap();
    api_client::Client::new(
        address.clone(),
        &api_client::AuthToken::Token(registration_response.session),
        5,
        30,
        &Default::default(),
        caps,
    )
    .unwrap()
}

#[allow(dead_code)]
pub async fn login(address: &url::Url, username: String, password: String) -> api_client::Client {
    // registration works
    let login_response = api_client::login(
        address,
        atuin_domain::api::LoginRequest { username, password },
        &Default::default(),
    )
    .await
    .unwrap();

    let caps = api_client::caps_client_anonymous(address, &Default::default()).unwrap();
    api_client::Client::new(
        address.clone(),
        &api_client::AuthToken::Token(login_response.session),
        5,
        30,
        &Default::default(),
        caps,
    )
    .unwrap()
}

#[allow(dead_code)]
pub async fn register(address: &url::Url) -> api_client::Client {
    let username = uuid_v7().as_simple().to_string();
    let password = uuid_v7().as_simple().to_string();
    register_inner(address, &username, &password).await
}
