//! `atuin workspace`: the `.atuin.lua` config file of a project.

use std::path::{Path, PathBuf};
use std::{env, io};

use atuin_client::lua_config::{Config, ConfigFile, OrgSlug, Orgs};
use atuin_common::dirs;
use atuin_common::path::AbsPathBuf;
use clap::{Args, Subcommand, ValueEnum};
use clap_couture::Couture;
use eyre::{Result, WrapErr as _};
use serde::Serialize;

use crate::i18n::fl;

#[derive(Subcommand, Debug, Couture)]
pub enum Cmd {
    #[command(about = fl!("cmd-workspace-init"), long_about = fl!("cmd-workspace-init", "long"))]
    Init(InitCmd),
    #[command(about = fl!("cmd-workspace-orgs"))]
    Orgs(OrgsCmd),
}

impl Cmd {
    pub fn run(self) -> Result<()> {
        match self {
            Self::Init(init) => init.run(),
            Self::Orgs(orgs) => orgs.run(),
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

#[derive(Args, Debug, Couture)]
pub struct OrgsCmd {
    #[arg(value_name = "DIR", help = fl!("arg-workspace-orgs-dir"))]
    dir: Option<PathBuf>,

    #[arg(long, value_enum, default_value_t = Style::Plain, help = fl!("arg-workspace-orgs-style"))]
    style: Style,
}

#[derive(ValueEnum, Debug, Clone, Copy)]
enum Style {
    Plain,
    Json,
}

/// Resolved org sharing for a directory; serialized directly for `--style json`.
#[derive(Debug, Serialize)]
struct Sharing {
    orgs: Vec<String>,
    /// Config file that set `orgs`, if any.
    config: Option<PathBuf>,
}

impl OrgsCmd {
    fn run(self) -> Result<()> {
        let dir = match self.dir {
            Some(dir) => dir,
            None => env::current_dir().wrap_err("failed to read the current directory")?,
        };
        // Resolve relative paths and `..` so ancestor lookup reaches parent dirs, while keeping
        // symlinks unresolved to match how the daemon records thread directories.
        let dir = AbsPathBuf::from_current_dir(&dir)
            .wrap_err_with(|| format!("failed to resolve {}", dir.display()))?
            .normalize_lexically();
        let home = dirs::try_atuin_home().ok().map(PathBuf::from);
        let file = ConfigFile::sharing(&dir, home.as_deref())?;
        let sharing = Sharing {
            config: file.as_ref().map(|file| file.path().to_owned()),
            orgs: file
                .and_then(|file| file.into_config().orgs)
                .map(|orgs| orgs.iter().map(ToString::to_string).collect())
                .unwrap_or_default(),
        };

        match self.style {
            Style::Json => {
                serde_json::to_writer(io::stdout().lock(), &sharing)?;
                println!();
            }
            Style::Plain => println!("{}", sharing.describe(&dir)),
        }
        Ok(())
    }
}

impl Sharing {
    fn describe(&self, dir: &Path) -> String {
        let dir = dir.display().to_string();
        let Some(path) = &self.config else {
            return fl!("workspace-orgs-none", dir = dir);
        };
        let path = path.display().to_string();
        if self.orgs.is_empty() {
            return fl!("workspace-orgs-nobody", path = path, dir = dir);
        }
        fl!("workspace-orgs-shared", path = path, dir = dir, orgs = self.orgs.join(", "))
    }
}
