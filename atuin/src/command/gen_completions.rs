use clap::{CommandFactory, Parser, ValueEnum};
use clap_complete::{generate, generate_to, Generator, Shell};
use clap_complete_nushell::Nushell;
use eyre::Result;

// clap put nushell completions into a seperate package due to the maintainers
// being a little less commited to support them.
// This means we have to do a tiny bit of legwork to combine these completions
// into one command.
#[derive(Debug, Clone, ValueEnum)]
#[value(rename_all = "lower")]
pub enum GenShell {
    Bash,
    Elvish,
    Fish,
    Nushell,
    PowerShell,
    Zsh,
}

impl Generator for GenShell {
    fn file_name(&self, name: &str) -> String {
        match self {
            // clap_complete
            Self::Bash => Shell::Bash.file_name(name),
            Self::Elvish => Shell::Elvish.file_name(name),
            Self::Fish => Shell::Fish.file_name(name),
            Self::PowerShell => Shell::PowerShell.file_name(name),
            Self::Zsh => Shell::Zsh.file_name(name),

            // clap_complete_nushell
            Self::Nushell => Nushell.file_name(name),
        }
    }

    fn generate(&self, cmd: &clap::Command, buf: &mut dyn std::io::prelude::Write) {
        match self {
            // clap_complete
            Self::Bash => Shell::Bash.generate(cmd, buf),
            Self::Elvish => Shell::Elvish.generate(cmd, buf),
            Self::Fish => Shell::Fish.generate(cmd, buf),
            Self::PowerShell => Shell::PowerShell.generate(cmd, buf),
            Self::Zsh => Shell::Zsh.generate(cmd, buf),

            // clap_complete_nushell
            Self::Nushell => Nushell.generate(cmd, buf),
        }
    }
}

#[derive(Debug, Parser)]
pub struct Cmd {
    /// Set the shell for generating completions
    #[arg(long, short)]
    shell: GenShell,

    /// Set the output directory
    #[arg(long, short)]
    out_dir: Option<String>,
}

impl Cmd {
    pub fn run(self) -> Result<()> {
        let Cmd { shell, out_dir } = self;

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
