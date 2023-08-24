#![forbid(unsafe_code)]

use std::{
    future::Future,
    net::{IpAddr, SocketAddr, TcpListener},
};

use atuin_server_database::Database;
use axum::Server;
use eyre::{Context, Result};

mod handlers;
mod router;
mod settings;
mod utils;

pub use settings::Settings;
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
    host: String,
    port: u16,
) -> Result<()> {
    let host = host.parse::<IpAddr>()?;
    launch_with_listener::<Db>(
        settings,
        TcpListener::bind(SocketAddr::new(host, port)).context("could not connect to socket")?,
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
