#[cfg(feature = "atuin-server-postgres")]
use atuin_server_postgres::{Postgres, PostgresSettings};
#[cfg(feature = "atuin-server-sqlite")]
use atuin_server_sqlite::{Sqlite, SqliteSettings};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

use clap::Parser;
use eyre::{Context, Result};

use atuin_server::Settings;

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

#[cfg(feature = "atuin-server-postgres")]
async fn launch(settings: Settings<PostgresSettings>, host: String, port: u16) -> Result<()> {
    atuin_server::launch::<Postgres>(settings, host, port).await
}

#[cfg(feature = "atuin-server-sqlite")]
async fn launch(settings: Settings<SqliteSettings>, host: String, port: u16) -> Result<()> {
    atuin_server::launch::<Sqlite>(settings, host, port).await
}
