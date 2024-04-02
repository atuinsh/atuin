use clap::Subcommand;
use eyre::Result;

#[cfg(not(windows))]
use rustix::{fs::Mode, process::umask};

#[cfg(feature = "client")]
mod client;

#[cfg(feature = "server")]
mod server;

mod contributors;

mod gen_completions;

#[derive(Subcommand)]
#[command(infer_subcommands = true)]
pub enum AtuinCmd {
    #[cfg(feature = "client")]
    #[command(flatten)]
    Client(client::Cmd),

    /// Start an atuin server
    #[cfg(feature = "server")]
    #[command(subcommand)]
    Server(server::Cmd),

    /// Generate a UUID
    Uuid,

    Contributors,

    /// Generate shell completions
    GenCompletions(gen_completions::Cmd),
}

impl AtuinCmd {
    pub fn run(self) -> Result<()> {
        #[cfg(not(windows))]
        {
            // set umask before we potentially open/create files
            // or in other words, 077. Do not allow any access to any other user
            let mode = Mode::RWXG | Mode::RWXO;
            umask(mode);
        }

        match self {
            #[cfg(feature = "client")]
            Self::Client(client) => client.run(),

            #[cfg(feature = "server")]
            Self::Server(server) => server.run(),
            Self::Contributors => {
                contributors::run();
                Ok(())
            }
            Self::Uuid => {
                println!("{}", atuin_common::utils::uuid_v7().as_simple());
                Ok(())
            }
            Self::GenCompletions(gen_completions) => gen_completions.run(),
        }
    }
}
