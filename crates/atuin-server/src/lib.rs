#![forbid(unsafe_code)]

use std::future::Future;
use std::net::SocketAddr;

use atuin_common::db::DbUrl;
use axum::{Router, serve};
use eyre::{Context, Result};

use crate::db::{Database, MySql, Postgres, Sqlite};

mod handlers;
mod metrics;
mod router;
mod trace;

pub mod db;

pub use settings::{Settings, example_config};

pub mod settings;

use tokio::net::TcpListener;
use tokio::signal;

#[cfg(target_family = "unix")]
async fn shutdown_signal() {
    let mut term = signal::unix::signal(signal::unix::SignalKind::terminate())
        .expect("failed to register signal handler");
    let mut interrupt = signal::unix::signal(signal::unix::SignalKind::interrupt())
        .expect("failed to register signal handler");

    tokio::select! {
        _ = term.recv() => {},
        _ = interrupt.recv() => {},
    };
    eprintln!("Shutting down gracefully...");
}

#[cfg(target_family = "windows")]
async fn shutdown_signal() {
    signal::windows::ctrl_c().expect("failed to register signal handler").recv().await;
    eprintln!("Shutting down gracefully...");
}

pub async fn launch(settings: Settings, addr: SocketAddr) -> Result<()> {
    launch_with_tcp_listener(
        settings,
        TcpListener::bind(addr).await.context("could not connect to socket")?,
        shutdown_signal(),
    )
    .await
}

pub async fn launch_with_tcp_listener(
    settings: Settings,
    listener: TcpListener,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> Result<()> {
    let router = connect_and_build_router(settings).await?;

    serve(listener, router.into_make_service_with_connect_info::<SocketAddr>())
        .with_graceful_shutdown(shutdown)
        .await?;

    Ok(())
}

/// Pick the backend from the connection URL and connect it.
///
/// Backend selection and connection are one and the same match: each arm hands its
/// backend a URL of exactly the right type, so pairing the wrong backend with a URL
/// is a compile error rather than a runtime check.
async fn connect_and_build_router(settings: Settings) -> Result<Router> {
    let db_uri = settings.db_settings.db_uri.clone();

    let router = match db_uri {
        DbUrl::Sqlite(url) => {
            let db = Sqlite::connect(url)
                .await
                .wrap_err_with(|| format!("failed to connect to db: {:?}", settings.db_settings))?;
            router::router(db, settings)
        }
        DbUrl::Postgres(url) => {
            let db = Postgres::connect(url)
                .await
                .wrap_err_with(|| format!("failed to connect to db: {:?}", settings.db_settings))?;
            router::router(db, settings)
        }
        DbUrl::Mysql(url) => {
            let db = MySql::connect(url)
                .await
                .wrap_err_with(|| format!("failed to connect to db: {:?}", settings.db_settings))?;
            router::router(db, settings)
        }
    };

    Ok(router)
}

// The separate listener means it's much easier to ensure metrics are not accidentally exposed to
// the public.
pub async fn launch_metrics_server(host: String, port: u16) -> Result<()> {
    let listener = TcpListener::bind((host, port)).await.context("failed to bind metrics tcp")?;

    let recorder_handle = metrics::setup_metrics_recorder();

    let router = Router::new().route(
        "/metrics",
        axum::routing::get(move || std::future::ready(recorder_handle.render())),
    );

    serve(listener, router.into_make_service()).with_graceful_shutdown(shutdown_signal()).await?;

    Ok(())
}
