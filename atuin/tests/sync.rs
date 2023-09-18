use std::{env, net::TcpListener, time::Duration};

use atuin_client::api_client;
use atuin_common::utils::uuid_v7;
use atuin_server::{launch_with_listener, Settings as ServerSettings};
use atuin_server_postgres::{Postgres, PostgresSettings};
use futures_util::TryFutureExt;
use tokio::{sync::oneshot, task::JoinHandle};
use tracing::{dispatcher, Dispatch};
use tracing_subscriber::{layer::SubscriberExt, EnvFilter};

async fn start_server(path: &str) -> (String, oneshot::Sender<()>, JoinHandle<()>) {
    let formatting_layer = tracing_tree::HierarchicalLayer::default()
        .with_writer(tracing_subscriber::fmt::TestWriter::new())
        .with_indent_lines(true)
        .with_ansi(true)
        .with_targets(true)
        .with_indent_amount(2);

    let dispatch: Dispatch = tracing_subscriber::registry()
        .with(formatting_layer)
        .with(EnvFilter::new("atuin_server=debug,info"))
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
    };

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let _tracing_guard = dispatcher::set_default(&dispatch);

        if let Err(e) = launch_with_listener::<Postgres>(
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

#[tokio::test]
async fn registration() {
    let path = format!("/{}", uuid_v7().as_simple());
    let (address, shutdown, server) = start_server(&path).await;
    dbg!(&address);

    let username = uuid_v7().as_simple().to_string();
    let email = format!("{}@example.com", uuid_v7().as_simple());
    let password = uuid_v7().as_simple().to_string();

    // registration works
    let registration_response = api_client::register(&address, &username, &email, &password)
        .await
        .unwrap();

    let client = api_client::Client::new(&address, &registration_response.session, 5, 30).unwrap();

    // the session token works
    let status = client.status().await.unwrap();
    assert_eq!(status.username, username);

    // login works
    let login_response = api_client::login(
        &address,
        atuin_common::api::LoginRequest { username, password },
    )
    .await
    .unwrap();

    // currently we return the same session token
    assert_eq!(registration_response.session, login_response.session);

    shutdown.send(()).unwrap();
    server.await.unwrap();
}
