#![forbid(unsafe_code)]

use std::net::{IpAddr, SocketAddr};

use axum::Server;
use database::Postgres;
use eyre::Result;

use crate::settings::Settings;

#[macro_use]
extern crate log;

#[macro_use]
extern crate serde_derive;

pub mod auth;
pub mod database;
pub mod handlers;
pub mod models;
pub mod router;
pub mod settings;

pub async fn launch(settings: Settings, host: String, port: u16) -> Result<()> {
    let host = host.parse::<IpAddr>()?;

    let postgres = Postgres::new(settings.db_uri.as_str()).await?;

    let r = router::router(postgres, settings);

    Server::bind(&SocketAddr::new(host, port))
        .serve(r.into_make_service())
        .await?;

    Ok(())
}
