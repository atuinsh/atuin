use std::env;

use eyre::{eyre, Result};

fn init_zsh() {
    let full = include_str!("../shell/atuin.zsh");
    println!("{}", full);
}

pub fn init() -> Result<()> {
    let shell = env::var("SHELL")?;

    if shell.ends_with("zsh") {
        init_zsh();
        Ok(())
    } else {
        Err(eyre!("Could not detect shell, or shell unsupported"))
    }
}
