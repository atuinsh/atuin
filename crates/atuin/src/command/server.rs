use std::net::SocketAddr;

use atuin_server_database::DbType;
use atuin_server_postgres::Postgres;
use atuin_server_sqlite::Sqlite;
use tracing_subscriber::{EnvFilter, fmt, prelude::*};

use clap::Parser;
use eyre::{Context, Result, eyre};

use atuin_server::{Settings, example_config, launch, launch_metrics_server};

#[derive(Parser, Debug)]
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

    /// Print server example configuration
    DefaultConfig,
}

impl Cmd {
    #[tokio::main]
    pub async fn run(self) -> Result<()> {
        tracing_subscriber::registry()
            .with(fmt::layer())
            .with(EnvFilter::from_default_env())
            .init();

        tracing::trace!(command = ?self, "server command");

        match self {
            Self::Start { host, port } => {
                let settings = Settings::new().wrap_err("could not load server settings")?;
                let host = host.as_ref().unwrap_or(&settings.host).clone();
                let port = port.unwrap_or(settings.port);
                let addr = SocketAddr::new(host.parse()?, port);

                if settings.metrics.enable {
                    tokio::spawn(launch_metrics_server(
                        settings.metrics.host.clone(),
                        settings.metrics.port,
                    ));
                }

                match settings.db_settings.db_type() {
                    DbType::Postgres => launch::<Postgres>(settings, addr).await,
                    DbType::Sqlite => launch::<Sqlite>(settings, addr).await,
                    DbType::Unknown => {
                        Err(eyre!("db_uri must start with postgres:// or sqlite://"))
                    }
                }
            }
            Self::DefaultConfig => {
                println!("{}", example_config());
                Ok(())
            }
        }
    }
}
