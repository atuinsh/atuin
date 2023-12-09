#![forbid(unsafe_code)]

use std::{future::Future, net::TcpListener};

use atuin_server_database::Database;
use axum::Router;
use axum::Server;
use eyre::{Context, Result};

mod handlers;
mod metrics;
mod router;
mod utils;

pub use settings::example_config;
pub use settings::Settings;

pub mod settings;

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
    signal::windows::ctrl_c()
        .expect("failed to register signal handler")
        .recv()
        .await;
    eprintln!("Shutting down gracefully...");
}

pub async fn launch<Db: Database>(
    settings: Settings<Db::Settings>,
    host: &str,
    port: u16,
) -> Result<()> {
    launch_with_listener::<Db>(
        settings,
        TcpListener::bind((host, port)).context("could not connect to socket")?,
        shutdown_signal(),
    )
    .await
}

pub async fn launch_with_listener<Db: Database>(
    settings: Settings<Db::Settings>,
    listener: TcpListener,
    shutdown: impl Future<Output = ()>,
) -> Result<()> {
    let db = Db::new(&settings.db_settings)
        .await
        .wrap_err_with(|| format!("failed to connect to db: {:?}", settings.db_settings))?;

    let r = router::router(db, settings);

    Server::from_tcp(listener)
        .context("could not launch server")?
        .serve(r.into_make_service())
        .with_graceful_shutdown(shutdown)
        .await?;

    Ok(())
}

// The separate listener means it's much easier to ensure metrics are not accidentally exposed to
// the public.
pub async fn launch_metrics_server(host: String, port: u16) -> Result<()> {
    let listener = TcpListener::bind((host, port)).context("failed to bind metrics tcp")?;

    let recorder_handle = metrics::setup_metrics_recorder();

    let router = Router::new().route(
        "/metrics",
        axum::routing::get(move || std::future::ready(recorder_handle.render())),
    );

    Server::from_tcp(listener)
        .context("could not launch server")?
        .serve(router.into_make_service())
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    Ok(())
}
