use eyre::Result;
use structopt::StructOpt;

#[derive(StructOpt)]
pub enum Cmd {
    #[structopt(about = "zsh setup")]
    Zsh,
    #[structopt(about = "bash setup")]
    Bash,
    #[structopt(about = "nu setup")]
    Nushell,
}

fn init_zsh() {
    let full = include_str!("../shell/atuin.zsh");
    println!("{}", full);
}

fn init_bash() {
    let full = include_str!("../shell/atuin.bash");
    println!("{}", full);
}

fn init_nu() {
    let full = include_str!("../shell/atuin.nu");
    println!("{}", full);
}

impl Cmd {
    pub fn run(&self) -> Result<()> {
        match self {
            Self::Zsh => init_zsh(),
            Self::Bash => init_bash(),
            Self::Nushell => init_nu(),
        }
        Ok(())
    }
}
