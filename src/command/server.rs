use clap::Subcommand;
use eyre::Result;

use atuin_server::launch;
use atuin_server::settings::Settings;

#[derive(Subcommand)]
pub enum Cmd {
    #[clap(
        about="starts the server",
        aliases=&["s", "st", "sta", "star"],
    )]
    Start {
        /// specify the host address to bind
        #[clap(long, short)]
        host: Option<String>,

        /// specify the port to bind
        #[clap(long, short)]
        port: Option<u16>,
    },
}

impl Cmd {
    pub async fn run(&self, settings: &Settings) -> Result<()> {
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
