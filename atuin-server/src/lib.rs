#![forbid(unsafe_code)]

use std::net::{IpAddr, SocketAddr};

use axum::Server;
use database::{Postgres, Sqlite};
use eyre::{Context, Result};

use crate::settings::Settings;

pub mod auth;
pub mod calendar;
pub mod database;
pub mod handlers;
pub mod models;
pub mod router;
pub mod settings;

pub async fn launch(settings: Settings, host: String, port: u16) -> Result<()> {
    let host = host.parse::<IpAddr>()?;

    let router = if settings.db_uri.starts_with("sqlite://") {
        let sqlite = Sqlite::new(settings.clone())
            .await
            .wrap_err_with(|| format!("failed to connect to db: {}", settings.db_uri))?;
        router::router(sqlite, settings)
    } else {
        let postgres = Postgres::new(settings.clone())
            .await
            .wrap_err_with(|| format!("failed to connect to db: {}", settings.db_uri))?;
        router::router(postgres, settings)
    };

    Server::bind(&SocketAddr::new(host, port))
        .serve(router.into_make_service())
        .await?;

    Ok(())
}
