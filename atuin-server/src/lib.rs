#![forbid(unsafe_code)]

use std::net::{IpAddr, SocketAddr};

use axum::Server;
use database::Postgres;
use eyre::{Context, Result};

use crate::settings::Settings;

use tokio::signal;

pub mod auth;
pub mod calendar;
pub mod database;
pub mod handlers;
pub mod models;
pub mod router;
pub mod settings;
pub mod utils;

async fn shutdown_signal() {
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to register signal handler")
            .recv()
            .await;
    };

    tokio::select! {
        _ = terminate => (),
    }
    eprintln!("Shutting down gracefully...");
}

pub async fn launch(settings: Settings, host: String, port: u16) -> Result<()> {
    let host = host.parse::<IpAddr>()?;

    let postgres = Postgres::new(settings.clone())
        .await
        .wrap_err_with(|| format!("failed to connect to db: {}", settings.db_uri))?;

    let r = router::router(postgres, settings);

    Server::bind(&SocketAddr::new(host, port))
        .serve(r.into_make_service())
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    Ok(())
}
