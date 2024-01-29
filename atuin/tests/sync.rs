use std::{env, time::Duration};

use atuin_client::api_client;
use atuin_common::{api::AddHistoryRequest, utils::uuid_v7};
use atuin_server::{launch_with_tcp_listener, Settings as ServerSettings};
use atuin_server_postgres::{Postgres, PostgresSettings};
use futures_util::TryFutureExt;
use time::OffsetDateTime;
use tokio::{net::TcpListener, sync::oneshot, task::JoinHandle};
use tracing::{dispatcher, Dispatch};
use tracing_subscriber::{layer::SubscriberExt, EnvFilter};

mod common;

#[tokio::test]
async fn sync() {
    let path = format!("/{}", uuid_v7().as_simple());
    let (address, shutdown, server) = common::start_server(&path).await;

    let client = common::register(&address).await;
    let hostname = uuid_v7().as_simple().to_string();
    let now = OffsetDateTime::now_utc();

    let data1 = uuid_v7().as_simple().to_string();
    let data2 = uuid_v7().as_simple().to_string();

    client
        .post_history(&[
            AddHistoryRequest {
                id: uuid_v7().as_simple().to_string(),
                timestamp: now,
                data: data1.clone(),
                hostname: hostname.clone(),
            },
            AddHistoryRequest {
                id: uuid_v7().as_simple().to_string(),
                timestamp: now,
                data: data2.clone(),
                hostname: hostname.clone(),
            },
        ])
        .await
        .unwrap();

    let history = client
        .get_history(OffsetDateTime::UNIX_EPOCH, OffsetDateTime::UNIX_EPOCH, None)
        .await
        .unwrap();

    assert_eq!(history.history, vec![data1, data2]);

    shutdown.send(()).unwrap();
    server.await.unwrap();
}
