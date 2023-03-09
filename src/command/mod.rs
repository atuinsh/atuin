use clap::{CommandFactory, Subcommand};
use clap_complete::{generate, generate_to, Shell};
use eyre::Result;

#[cfg(feature = "client")]
mod client;

mod init;

mod contributors;

#[derive(Subcommand)]
#[command(infer_subcommands = true)]
pub enum AtuinCmd {
    #[cfg(feature = "client")]
    #[command(flatten)]
    Client(client::Cmd),

    /// Start an atuin server
    #[cfg(feature = "server")]
    #[command(subcommand)]
    Server(atuin_server::Cmd),

    /// Output shell setup
    Init(init::Cmd),

    /// Generate a UUID
    Uuid,

    Contributors,

    /// Generate shell completions
    GenCompletions {
        /// Set the shell for generating completions
        #[arg(long, short)]
        shell: Shell,

        /// Set the output directory
        #[arg(long, short)]
        out_dir: Option<String>,
    },
}

impl AtuinCmd {
    pub fn run(self) -> Result<()> {
        match self {
            #[cfg(feature = "client")]
            Self::Client(client) => client.run(),
            #[cfg(feature = "server")]
            Self::Server(server) => server.run(),
            Self::Contributors => {
                contributors::run();
                Ok(())
            }
            Self::Init(init) => {
                init.run();
                Ok(())
            }
            Self::Uuid => {
                println!("{}", atuin_common::utils::uuid_v4());
                Ok(())
            }
            Self::GenCompletions { shell, out_dir } => {
                let mut cli = crate::Atuin::command();

                match out_dir {
                    Some(out_dir) => {
                        generate_to(shell, &mut cli, env!("CARGO_PKG_NAME"), &out_dir)?;
                    }
                    None => {
                        generate(
                            shell,
                            &mut cli,
                            env!("CARGO_PKG_NAME"),
                            &mut std::io::stdout(),
                        );
                    }
                }

                Ok(())
            }
        }
    }
}
