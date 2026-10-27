#![forbid(unsafe_code)]

use std::io::{self, Write as _};
use std::net::SocketAddr;

use atuin_common::logs::USER;
use atuin_server::admin::AdminCmd;
use atuin_server::{Settings, example_config, launch, launch_metrics_server};
use clap::Parser;
use eyre::{Context, Result};
use tracing::Level;
use tracing_subscriber::filter::{LevelFilter, filter_fn};
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

    /// Manage the server by hand
    #[clap(subcommand)]
    Admin(AdminCmd),
}

#[tokio::main]
async fn main() -> Result<()> {
    let cmd = Cmd::parse();

    // Warnings show without RUST_LOG, since that is how loading the settings reports what it
    // ignored. A message for the operator, such as an admin command saying what it did, is a
    // plain line on stderr instead.
    let filter = EnvFilter::builder()
        .with_default_directive(LevelFilter::WARN.into())
        .from_env_lossy()
        .add_directive(format!("{USER}=warn").parse()?);
    let user = fmt::layer()
        .with_writer(std::io::stderr)
        .without_time()
        .with_level(false)
        .with_target(false)
        .with_filter(filter_fn(|meta| meta.target() == USER && *meta.level() == Level::INFO));
    tracing_subscriber::registry().with(fmt::layer().with_filter(filter)).with(user).init();

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
            writeln!(io::stdout(), "{}", example_config())?;
            Ok(())
        }
        Cmd::Admin(cmd) => {
            let settings = Settings::new().wrap_err("could not load server settings")?;
            cmd.run(&settings.db_settings).await
        }
    }
}
