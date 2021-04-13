use eyre::Result;
use structopt::StructOpt;

use crate::remote::server;
use crate::settings::Settings;

#[derive(StructOpt)]
pub enum Cmd {
    #[structopt(
        about="starts the server",
        aliases=&["s", "st", "sta", "star"],
    )]
    Start {
        #[structopt(about = "specify the host address to bind", long, short)]
        host: Option<String>,

        #[structopt(about = "specify the port to bind", long, short)]
        port: Option<u16>,
    },
}

impl Cmd {
    pub fn run(&self, settings: &Settings) -> Result<()> {
        match self {
            Self::Start { host, port } => {
                let host = host.as_ref().map_or(
                    settings.server.host.clone(),
                    std::string::ToString::to_string,
                );
                let port = port.map_or(settings.server.port, |p| p);

                server::launch(settings, host, port);
            }
        }
        Ok(())
    }
}
