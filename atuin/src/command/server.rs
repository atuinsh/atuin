use atuin_server_postgres::Postgres;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

use clap::Parser;
use eyre::{Context, Result};

use atuin_server::{example_config, launch, Settings};

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

                launch::<Postgres>(settings, &host, port).await
            }
            Self::DefaultConfig => {
                println!("{}", example_config());
                Ok(())
            }
        }
    }
}
