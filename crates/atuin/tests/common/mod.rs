use std::{env, time::Duration};

use atuin_client::api_client;
use atuin_common::utils::uuid_v7;
use atuin_server::{launch_with_tcp_listener, Settings as ServerSettings};
use atuin_server_postgres::{Postgres, PostgresSettings};
use futures_util::TryFutureExt;
use tokio::{net::TcpListener, sync::oneshot, task::JoinHandle};
use tracing::{dispatcher, Dispatch};
use tracing_subscriber::{layer::SubscriberExt, EnvFilter};

pub async fn start_server(path: &str) -> (String, oneshot::Sender<()>, JoinHandle<()>) {
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
        max_history_length: 8192,
        max_record_size: 1024 * 1024 * 1024,
        page_size: 1100,
        register_webhook_url: None,
        register_webhook_username: String::new(),
        db_settings: PostgresSettings { db_uri },
        metrics: atuin_server::settings::Metrics::default(),
        tls: atuin_server::settings::Tls::default(),
        mail: atuin_server::settings::Mail::default(),
        fake_version: None,
    };

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let _tracing_guard = dispatcher::set_default(&dispatch);

        if let Err(e) = launch_with_tcp_listener::<Postgres>(
            server_settings,
            listener,
            shutdown_rx.unwrap_or_else(|_| ()),
        )
        .await
        {
            tracing::error!(error=?e, "server error");
            panic!("error running server: {e:?}");
        }
    });

    // let the server come online
    tokio::time::sleep(Duration::from_millis(200)).await;

    (format!("http://{addr}{path}"), shutdown_tx, server)
}

pub async fn register_inner<'a>(
    address: &'a str,
    username: &str,
    password: &str,
) -> api_client::Client<'a> {
    let email = format!("{}@example.com", uuid_v7().as_simple());

    // registration works
    let registration_response = api_client::register(address, username, &email, password)
        .await
        .unwrap();

    api_client::Client::new(address, &registration_response.session, 5, 30).unwrap()
}

#[allow(dead_code)]
pub async fn login(address: &str, username: String, password: String) -> api_client::Client<'_> {
    // registration works
    let login_respose = api_client::login(
        address,
        atuin_common::api::LoginRequest { username, password },
    )
    .await
    .unwrap();

    api_client::Client::new(address, &login_respose.session, 5, 30).unwrap()
}

#[allow(dead_code)]
pub async fn register(address: &str) -> api_client::Client<'_> {
    let username = uuid_v7().as_simple().to_string();
    let password = uuid_v7().as_simple().to_string();
    register_inner(address, &username, &password).await
}
