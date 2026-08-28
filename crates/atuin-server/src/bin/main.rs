#![forbid(unsafe_code)]

use std::net::SocketAddr;

use atuin_server::{Settings, example_config, launch, launch_metrics_server};
use clap::Parser;
use eyre::{Context, Result};
use tracing_subscriber::prelude::*;
use tracing_subscriber::{EnvFilter, fmt};

#[derive(Parser, Debug)]
#[clap(name = "atuin-server", about = "Atuin sync server", version, infer_subcommands = true)]
enum Cmd {
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

#[tokio::main]
async fn main() -> Result<()> {
    let cmd = Cmd::parse();

    tracing_subscriber::registry().with(fmt::layer()).with(EnvFilter::from_default_env()).init();

    tracing::trace!(command = ?cmd, "server command");

    match cmd {
        Cmd::Start { host, port } => {
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

            launch(settings, addr).await
        }
        Cmd::DefaultConfig => {
            println!("{}", example_config());
            Ok(())
        }
    }
}
