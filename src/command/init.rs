use structopt::StructOpt;

#[derive(StructOpt)]
pub enum Cmd {
    #[structopt(about = "zsh setup")]
    Zsh,
    #[structopt(about = "bash setup")]
    Bash,
    #[structopt(about = "fish setup")]
    Fish,
}

fn init_zsh() {
    let full = include_str!("../shell/atuin.zsh");
    println!("{}", full);
}

fn init_bash() {
    let full = include_str!("../shell/atuin.bash");
    println!("{}", full);
}

fn init_fish() {
    let full = include_str!("../shell/atuin.fish");
    println!("{}", full);
}

impl Cmd {
    pub fn run(&self) {
        match self {
            Self::Zsh => init_zsh(),
            Self::Bash => init_bash(),
            Self::Fish => init_fish(),
        }
    }
}
