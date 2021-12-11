use eyre::Result;
use structopt::StructOpt;

use atuin_server::launch;
use atuin_server::settings::Settings;

#[derive(StructOpt)]
pub enum Cmd {
    #[structopt(
        about="starts the server",
        aliases=&["s", "st", "sta", "star"],
    )]
    Start {
        #[structopt(help = "specify the host address to bind", long, short)]
        host: Option<String>,

        #[structopt(help = "specify the port to bind", long, short)]
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
