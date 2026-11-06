//! Amp (`amp`): installing its plugin and resuming.
//!
//! Amp keeps its threads on its servers, not on disk, so there is nothing to watch. Atuin's
//! plugin for it converts the messages of each turn, as Amp's `agent.end` event hands them over,
//! and pushes them with `atuin threads push`.

use std::path::{Path, PathBuf};

use super::rehydrate::{RehydrateError, RehydrateSession};
use super::resume::{self, CwdRequirement, ResumeError, ResumePlan, ResumeTarget};
use super::session::any::{AnySessions, ObservableExt};
use super::{Harness, InstallHookError};
use crate::dirs::home_dir;
use crate::path::PathBufExt as _;
use crate::sync::BlockingPool;

pub mod export;

#[derive(Debug, Clone, Copy, Default)]
pub struct Amp;

impl Harness for Amp {
    fn name(&self) -> &'static str {
        "amp"
    }

    /// Amp loads every plugin in `~/.config/amp/plugins/` ([Plugin API]).
    ///
    /// [Plugin API]: https://ampcode.com/docs/plugin-api
    async fn install_hooks(&self, _executable: &Path) -> Result<PathBuf, InstallHookError> {
        const PLUGIN_SOURCE: &str = include_str!("../../../res/harnesstools/amp/atuin.ts");

        let plugins_dir = home_dir().joined(".config").joined("amp").joined("plugins");
        let plugin_path = plugins_dir.clone().joined("atuin.ts");
        let installed = tokio::fs::read_to_string(&plugin_path).await;
        if installed.is_ok_and(|existing| existing == PLUGIN_SOURCE) {
            return Err(InstallHookError::AlreadyInstalled);
        }
        tokio::fs::create_dir_all(&plugins_dir).await?;
        tokio::fs::write(&plugin_path, PLUGIN_SOURCE).await?;
        Ok(plugin_path)
    }

    /// `amp threads continue <id>`: Amp fetches the thread from its servers, from anywhere, and
    /// works in the directory it runs from.
    fn resume_plan(&self, target: &ResumeTarget) -> Result<ResumePlan, ResumeError> {
        let args = ["threads".to_owned(), "continue".to_owned(), target.id.clone()];
        Ok(resume::plan(target, CwdRequirement::Preferred, "amp", args))
    }

    /// Never here: Amp's threads are on its servers.
    async fn locate(&self, _id: &str) -> Option<PathBuf> {
        None
    }

    /// Amp's threads are on its servers, which atuin can't write to; a thread from another
    /// machine is already there.
    async fn rehydrate(&self, _session: &RehydrateSession) -> Result<PathBuf, RehydrateError> {
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

    #[rstest]
    fn resumes_by_continuing_the_thread() {
        let target = ResumeTarget::new("T-01a19e1c-1bf0-70ab-ac5d-1b6d033ed2f0").with_cwd("/work");
        let plan = Amp.resume_plan(&target).unwrap();
        assert_eq!(plan.program, "amp");
        assert_eq!(plan.args, ["threads", "continue", "T-01a19e1c-1bf0-70ab-ac5d-1b6d033ed2f0"]);
    }
}
