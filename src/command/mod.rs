use clap::Subcommand;
use eyre::Result;

#[cfg(feature = "client")]
mod client;

#[cfg(feature = "server")]
mod server;

#[derive(Subcommand)]
#[clap(infer_subcommands = true)]
pub enum AtuinCmd {
    #[cfg(feature = "client")]
    #[clap(flatten)]
    Client(client::Cmd),

    /// Start an atuin server
    #[cfg(feature = "server")]
    #[clap(subcommand)]
    Server(server::Cmd),
}

impl AtuinCmd {
    pub fn run(self) -> Result<()> {
        match self {
            #[cfg(feature = "client")]
            Self::Client(client) => client.run(),
            #[cfg(feature = "server")]
            Self::Server(server) => server.run(),
        }
    }
}
