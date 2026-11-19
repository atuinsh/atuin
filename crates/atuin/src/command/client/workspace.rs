//! `atuin workspace`: the `.atuin.lua` config file of a project.

use std::env;

use atuin_client::lua_config::{Config, ConfigFile, OrgSlug, Orgs};
use clap::{Args, Subcommand};
use clap_couture::Couture;
use eyre::{Result, WrapErr as _};

use crate::i18n::fl;

#[derive(Subcommand, Debug, Couture)]
pub enum Cmd {
    #[command(about = fl!("cmd-workspace-init"), long_about = fl!("cmd-workspace-init", "long"))]
    Init(InitCmd),
}

impl Cmd {
    pub fn run(self) -> Result<()> {
        match self {
            Self::Init(init) => init.run(),
        }
    }
}

#[derive(Args, Debug, Couture)]
pub struct InitCmd {
    #[arg(long = "org", value_name = "SLUG", help = fl!("arg-workspace-init-org"))]
    #[couture(prompt = fl!("prompt-workspace-org"))]
    orgs: Vec<OrgSlug>,
}

impl InitCmd {
    fn run(self) -> Result<()> {
        let orgs = Orgs::try_from(self.orgs)?;
        // Not `utils::get_current_dir`, which trusts `$PWD`: a parent that sets the working
        // directory can leave `$PWD` stale, and this writes a file.
        let dir = env::current_dir().wrap_err("failed to read the current directory")?;
        let file = ConfigFile::create(&dir, Config { orgs: Some(orgs) })?;
        println!("{}", fl!("workspace-init-created", path = file.path().display().to_string()));
        Ok(())
    }
}
