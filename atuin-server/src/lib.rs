#![forbid(unsafe_code)]

use std::net::{IpAddr, SocketAddr};

use atuin_server_database::Database;
use axum::Server;
use eyre::{Context, Result};

mod handlers;
mod router;
mod settings;
mod utils;

pub use settings::Settings;
use tokio::signal;

async fn shutdown_signal() {
    signal::unix::signal(signal::unix::SignalKind::terminate())
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

    let db = Db::new(&settings.db_settings)
        .await
        .wrap_err_with(|| format!("failed to connect to db: {:?}", settings.db_settings))?;

    let r = router::router(db, settings);

    Server::bind(&SocketAddr::new(host, port))
        .serve(r.into_make_service())
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    Ok(())
}
