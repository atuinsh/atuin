use std::path::{Path, PathBuf};

use super::rehydrate::{RehydrateError, RehydrateSession};
use super::resume::{self, CwdRequirement, ResumeError, ResumePlan, ResumeTarget};
use super::{Harness, InstallHookError};
use crate::dirs::home_dir;
use crate::env::var_nonempty;
use crate::path::PathBufExt as _;

pub mod rehydrate;
pub mod session;

/// Which pi: pi itself, or oh-my-pi (`omp`), a fork that keeps pi's session format in a
/// directory of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flavour {
    Pi,
    Omp,
}

impl Flavour {
    /// The command that runs it, which is also its name.
    #[must_use]
    pub const fn binary(self) -> &'static str {
        match self {
            Self::Pi => "pi",
            Self::Omp => "omp",
        }
    }

    /// Its config directory: `PI_CODING_AGENT_DIR` (both read it), else `~/.pi/agent` (pi-mono
    /// `packages/coding-agent/src/config.ts` `getAgentDir`) or `~/.omp/agent`.
    pub(crate) fn agent_dir(self) -> PathBuf {
        var_nonempty("PI_CODING_AGENT_DIR").map_or_else(
            || home_dir().joined(format!(".{}", self.binary())).joined("agent"),
            |dir| expand_tilde(&dir),
        )
    }

    /// The model a session starts on, as `provider/id`, when one is configured and this flavour
    /// needs telling: omp's `modelRoles.default` (its `config.yml`). pi falls back to its own
    /// default by itself.
    fn default_model(self) -> Option<String> {
        if self == Self::Omp {
            omp_default_model(&self.agent_dir())
        } else {
            None
        }
    }
}

/// omp's `modelRoles.default` from `config.yml` (or `config.yaml`) in `agent_dir`.
fn omp_default_model(agent_dir: &Path) -> Option<String> {
    let config = ["config.yml", "config.yaml"]
        .iter()
        .find_map(|name| std::fs::read_to_string(agent_dir.join(name)).ok())?;
    let docs = yaml_rust2::YamlLoader::load_from_str(&config).ok()?;
    let model = docs.first()?["modelRoles"]["default"].as_str()?.trim();
    model.contains('/').then(|| model.to_owned())
}

/// Expand a leading `~` the way pi does for its path settings and variables (pi-mono
/// `packages/coding-agent/src/utils/paths.ts` `normalizePath`): `~` alone and `~/...`, nothing
/// else.
fn expand_tilde(path: &std::ffi::OsStr) -> PathBuf {
    let path = Path::new(path);
    match path.strip_prefix("~") {
        Ok(rest)
            if path.to_str().is_some_and(|p| {
                p == "~" || p.starts_with("~/") || (cfg!(windows) && p.starts_with("~\\"))
            }) =>
        {
            home_dir().joined(rest)
        }
        _ => path.to_path_buf(),
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Pi {
    flavour: Flavour,
}

impl Pi {
    pub const PI: Self = Self {
        flavour: Flavour::Pi,
    };
    pub const OMP: Self = Self {
        flavour: Flavour::Omp,
    };

    #[must_use]
    pub const fn flavour(self) -> Flavour {
        self.flavour
    }
}

impl Harness for Pi {
    fn name(&self) -> &'static str {
        self.flavour.binary()
    }

    /// Install the hooks on the current active user's machine
    async fn install_hooks(&self) -> Result<PathBuf, InstallHookError> {
        const PLUGIN_SOURCE: &str = include_str!("../../../res/harnesstools/pi/atuin.ts");
        const PLUGIN_NAME: &str = "atuin.ts";

        let extensions_dir = self.flavour.agent_dir().joined("extensions");

        tokio::fs::create_dir_all(&extensions_dir).await?;

        let extension_path = extensions_dir.joined(PLUGIN_NAME);
        // Commands are recorded with the harness's own name as their author.
        let source = PLUGIN_SOURCE.replace(
            r#"const ATUIN_AUTHOR = "pi";"#,
            &format!(r#"const ATUIN_AUTHOR = "{}";"#, self.flavour.binary()),
        );

        let already_installed = tokio::fs::read_to_string(&extension_path)
            .await
            .is_ok_and(|existing| existing == source);

        if already_installed {
            return Err(InstallHookError::AlreadyInstalled);
        }

        tokio::fs::write(&extension_path, source).await?;

        Ok(extension_path)
    }

    /// `pi --session <file>` when the session's file is known: pi opens a file in place from
    /// anywhere, and works in the directory its header names. Otherwise `pi --session <id>`,
    /// which pi resolves in the current directory's sessions first; one it only finds in another
    /// project's it offers to fork instead, so the session's own directory is required.
    fn resume_plan(&self, target: &ResumeTarget) -> Result<ResumePlan, ResumeError> {
        let (cwd, session) = match &target.native_path {
            Some(path) => (CwdRequirement::Preferred, resume::path_arg(path)?),
            None => (CwdRequirement::Required, target.id.clone()),
        };
        Ok(resume::plan(target, cwd, self.flavour.binary(), ["--session".to_owned(), session]))
    }

    async fn locate(&self, id: &str) -> Option<PathBuf> {
        let (root, id) = (session::default_root(self.flavour), id.to_owned());
        resume::blocking(move || session::locate(&root, &id)).await
    }

    async fn rehydrate(&self, session: &RehydrateSession) -> Result<PathBuf, RehydrateError> {
        rehydrate::rehydrate(self.flavour, session).await
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    /// omp's default model, from `modelRoles.default` in its `config.yml` or `config.yaml`.
    #[rstest]
    #[case::configured(
        "config.yml",
        "modelRoles:\n  default: openai-codex/gpt-5.6-luna\n",
        Some("openai-codex/gpt-5.6-luna")
    )]
    #[case::yaml_extension(
        "config.yaml",
        "modelRoles:\n  default: anthropic/claude-sonnet-5\n",
        Some("anthropic/claude-sonnet-5")
    )]
    #[case::no_default("config.yml", "setupVersion: 2\n", None)]
    #[case::no_provider("config.yml", "modelRoles:\n  default: gpt-5\n", None)]
    #[case::not_yaml("config.yml", "modelRoles: [\n", None)]
    fn omp_default_model_is_read_from_its_config(
        #[case] name: &str,
        #[case] config: &str,
        #[case] want: Option<&str>,
    ) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(name), config).unwrap();
        assert_eq!(omp_default_model(dir.path()).as_deref(), want);
    }
}
