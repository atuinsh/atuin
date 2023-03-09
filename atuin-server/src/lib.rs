#![forbid(unsafe_code)]

use std::net::{IpAddr, SocketAddr};

use axum::Server;
use clap::Parser;
use database::Postgres;
use eyre::{Context, Result};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

use crate::settings::Settings;

mod auth;
mod calendar;
mod database;
mod handlers;
mod models;
mod router;
mod settings;

async fn launch(settings: Settings, host: String, port: u16) -> Result<()> {
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

#[derive(Parser)]
#[clap(infer_subcommands = true)]
pub enum Cmd {
    /// Start the server
    Start {
        /// The host address to bind
        #[clap(long)]
        host: Option<String>,

        /// The port to bind
        #[clap(long, short)]
        port: Option<u16>,
    },
}

impl Cmd {
    #[tokio::main]
    pub async fn run(self) -> Result<()> {
        tracing_subscriber::registry()
            .with(fmt::layer())
            .with(EnvFilter::from_default_env())
            .init();

        let settings = Settings::new().wrap_err("could not load server settings")?;

        match self {
            Self::Start { host, port } => {
                let host = host
                    .as_ref()
                    .map_or(settings.host.clone(), std::string::ToString::to_string);
                let port = port.map_or(settings.port, |p| p);

                launch(settings, host, port).await
            }
        }
    }
}
