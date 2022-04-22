use clap::Subcommand;
use eyre::Result;

mod client;

#[cfg(feature = "server")]
mod server;

#[derive(Subcommand)]
#[clap(infer_subcommands = true)]
pub enum AtuinCmd {
    #[clap(flatten)]
    Client(client::Cmd),

    /// Start an atuin server
    #[cfg(feature = "server")]
    #[clap(subcommand)]
    Server(server::Cmd),
}

impl AtuinCmd {
    pub async fn run(self) -> Result<()> {
        match self {
            Self::Client(client) => client.run().await,
            #[cfg(feature = "server")]
            Self::Server(server) => server.run().await,
        }
    }
}
