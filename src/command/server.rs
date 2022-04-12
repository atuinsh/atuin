use clap::Parser;
use eyre::Result;

use atuin_server::launch;
use atuin_server::settings::Settings;

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
    pub async fn run(&self, settings: Settings) -> Result<()> {
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
