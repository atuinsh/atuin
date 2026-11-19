//! Amp (`amp`): installing its plugin and resuming.
//!
//! Amp keeps its threads on its servers, not on disk, so there is nothing to watch. Atuin's
//! plugin for it converts the messages of each turn, as Amp's `agent.end` event hands them over,
//! and pushes them with `atuin threads push`.

use std::path::PathBuf;

use super::rehydrate::{RehydrateError, RehydrateSession, Rehydrated};
use super::resume::{self, CwdRequirement, ResumeError, ResumePlan, ResumeTarget};
use super::session::any::{AnySessions, ObservableExt};
use super::{Harness, InstallHookError, TS_ATUIN_LINE, plugin_with_executable};
use crate::atuin_exe::launch_path;
use crate::dirs::home_dir;
use crate::env::var_nonempty;
use crate::path::PathBufExt as _;
use crate::sync::BlockingPool;

pub mod export;

#[derive(Debug, Clone, Copy, Default)]
pub struct Amp;

/// Amp's config directory, where it reads its settings and loads plugins from:
/// `$XDG_CONFIG_HOME/amp`, falling back to `~/.config/amp`.
pub(crate) fn config_dir() -> PathBuf {
    var_nonempty("XDG_CONFIG_HOME")
        .map_or_else(|| home_dir().joined(".config"), PathBuf::from)
        .joined("amp")
}

impl Harness for Amp {
    fn name(&self) -> &'static str {
        "amp"
    }

    fn display_name(&self) -> &'static str {
        "Amp"
    }

    fn program(&self) -> &'static str {
        "amp"
    }

    fn home_vars(&self) -> &'static [&'static str] {
        &["XDG_CONFIG_HOME"]
    }

    /// Amp loads every plugin in `plugins/` under its config directory, `$XDG_CONFIG_HOME/amp` or
    /// `~/.config/amp` ([Plugin API]).
    ///
    /// [Plugin API]: https://ampcode.com/docs/plugin-api
    async fn install_hooks(&self) -> Result<PathBuf, InstallHookError> {
        const PLUGIN_SOURCE: &str = include_str!("../../../res/harnesstools/amp/atuin.ts");

        let source = plugin_with_executable(PLUGIN_SOURCE, TS_ATUIN_LINE, launch_path().await?)?;
        let plugins_dir = config_dir().joined("plugins");
        let plugin_path = plugins_dir.clone().joined("atuin.ts");
        let installed = tokio::fs::read_to_string(&plugin_path).await;
        if installed.is_ok_and(|existing| existing == source) {
            return Err(InstallHookError::AlreadyInstalled);
        }
        tokio::fs::create_dir_all(&plugins_dir).await?;
        tokio::fs::write(&plugin_path, source).await?;
        Ok(plugin_path)
    }

    async fn has_hooks(&self) -> Result<bool, InstallHookError> {
        super::own_file_exists(&config_dir().joined("plugins").joined("atuin.ts")).await
    }

    async fn uninstall_hooks(&self) -> Result<bool, InstallHookError> {
        super::remove_own_file(&config_dir().joined("plugins").joined("atuin.ts")).await
    }

    /// `amp threads continue <id>`: Amp fetches the thread from its servers, from anywhere, and
    /// works in the directory it runs from.
    fn resume_plan(&self, target: &ResumeTarget) -> Result<ResumePlan, ResumeError> {
        let args = ["threads".to_owned(), "continue".to_owned(), target.id.clone()];
        Ok(resume::plan(target, CwdRequirement::Preferred, self.program(), args))
    }

    /// Never here: Amp's threads are on its servers.
    async fn locate(&self, _id: &str) -> Option<PathBuf> {
        None
    }

    /// Amp's threads are on its servers, which atuin can't write to; a thread from another
    /// machine is already there.
    async fn rehydrate(&self, _session: &RehydrateSession) -> Result<Rehydrated, RehydrateError> {
        Err(RehydrateError::Unsupported("Amp"))
    }
}

/// Amp pushes its threads (`atuin threads push`): there is nothing to watch.
impl ObservableExt for Amp {
    fn sessions(&self, _pool: &BlockingPool) -> Option<AnySessions> {
        None
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    /// The plugin goes where Amp loads plugins from, which `XDG_CONFIG_HOME` moves, and runs atuin
    /// by its launch path.
    #[rstest]
    #[case::default(None, ".config/amp/plugins/atuin.ts")]
    #[case::xdg(Some("xdg"), "xdg/amp/plugins/atuin.ts")]
    #[tokio::test]
    async fn installs_the_plugin_where_amp_loads_it(
        #[case] xdg: Option<&str>,
        #[case] relative: &str,
    ) {
        let home = tempfile::tempdir().unwrap();
        let env = crate::env::MockEnv::install();
        env.set("HOME", home.path());
        if let Some(xdg) = xdg {
            env.set("XDG_CONFIG_HOME", home.path().join(xdg));
        }

        let installed = Amp.install_hooks().await.unwrap();

        assert_eq!(installed, home.path().join(relative));
        let atuin = serde_json::Value::from(launch_path().await.unwrap().to_str().unwrap());
        let plugin = std::fs::read_to_string(installed).unwrap();
        assert!(plugin.contains(&format!("const ATUIN = {atuin};")), "{plugin}");
    }

    #[rstest]
    fn resumes_by_continuing_the_thread() {
        let target = ResumeTarget::new("T-01a19e1c-1bf0-70ab-ac5d-1b6d033ed2f0").with_cwd("/work");
        let plan = Amp.resume_plan(&target).unwrap();
        assert_eq!(plan.program, "amp");
        assert_eq!(plan.args, ["threads", "continue", "T-01a19e1c-1bf0-70ab-ac5d-1b6d033ed2f0"]);
    }
}
