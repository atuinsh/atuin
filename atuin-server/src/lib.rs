#![forbid(unsafe_code)]

use std::net::{IpAddr, SocketAddr};

use axum::Server;
use database::Postgres;
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

    let postgres = Postgres::new(settings.clone())
        .await
        .wrap_err_with(|| format!("failed to connect to db: {}", settings.db_uri))?;

    let r = router::router(postgres, settings);

    Server::bind(&SocketAddr::new(host, port))
        .serve(r.into_make_service())
        .await?;

    Ok(())
}
