use clap::Subcommand;
use eyre::Result;

mod client;
mod server;

#[derive(Subcommand)]
#[clap(infer_subcommands = true)]
pub enum AtuinCmd {
    #[clap(flatten)]
    Client(client::Cmd),

    /// Start an atuin server
    #[clap(subcommand)]
    Server(server::Cmd),
}

impl AtuinCmd {
    pub async fn run(self) -> Result<()> {
        match self {
            Self::Client(client) => client.run().await,
            Self::Server(server) => server.run().await,
        }
    }
}
