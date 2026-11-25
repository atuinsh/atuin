//! Tools for interacting and operating on different AI harnesses.

use std::path::{Path, PathBuf};

use enum_dispatch::enum_dispatch;

pub mod access;
pub mod amp;
pub mod ccode;
pub mod codex;
pub mod continuation;
pub mod copilot;
pub mod fork;
pub mod grok;
pub mod hermes;
mod json_hooks;
pub mod kiro;
pub mod mcp;
pub mod note;
pub mod opencode;
pub mod pi;
pub mod rehydrate;
pub mod resume;
pub mod session;
pub mod sync;
pub mod vibe;

use amp::Amp;
use ccode::Ccode;
use codex::Codex;
use copilot::Copilot;
use grok::Grok;
use hermes::Hermes;
use kiro::Kiro;
use mcp::{InstallMcp, InstallMcpError};
use opencode::Opencode;
use pi::Pi;
use rehydrate::{RehydrateError, RehydrateMessage, RehydrateSession, Rehydrated};
use resume::{ResumeError, ResumePlan, ResumeTarget};
use session::any::{AnySessions, ObservableExt};
use sync::{
    AppendOptions, AppendOutcome, Liveness, LocalTip, ReplaceOutcome, SessionSync, SyncError,
};
use vibe::Vibe;

use crate::sync::BlockingPool;

/// Defines a generic harness trait that all implementations need to implement.
#[enum_dispatch]
pub trait Harness: std::fmt::Debug {
    /// The well-defined name of this harness.
    fn name(&self) -> &'static str;

    /// Names in addition to [`Self::name`] which are considered to be the names this harness uses.
    ///
    /// There is a default implementation -- the empty set.
    fn alias_names(&self) -> &'static [&'static str] {
        &[]
    }

    /// Whether a session written out for this harness holds the one branch it resumes, rather
    /// than every row synced of it. A harness whose transcript keeps a tree resumes the branch its
    /// last line names, and leaves the rest as it is; one whose writer makes every row it's given
    /// part of the conversation says yes, so a turn undone elsewhere isn't brought back.
    fn writes_one_branch(&self) -> bool {
        false
    }

    /// Whether a restore of a session that is a fork writes the fork's link to the session it came
    /// from, as the fork itself does. A restore is written from synced rows, which name no fork.
    /// Capture keeps the restored copy a fork either way, as it takes the parent sync stored for a
    /// session over a continuation's marker the rows carry, so the link is for the harness's own
    /// view of the session. The other harnesses' restores of forks write no link.
    fn restores_fork_links(&self) -> bool {
        false
    }

    /// The harness's name as people know it. For example, setup says "Recording commands from Grok
    /// Build".
    fn display_name(&self) -> &'static str;

    /// The command that runs the harness. It is looked up on `PATH`.
    fn program(&self) -> &'static str;

    /// The environment variables that move where the harness keeps its sessions and config, as
    /// atuin finds them. A daemon that launchd starts gets them from the shell that installed its
    /// agent; without them it would read the harness's default home.
    fn home_vars(&self) -> &'static [&'static str];

    /// Install this harness's hooks on the current user's machine, returning the path written.
    #[allow(async_fn_in_trait)]
    async fn install_hooks(&self) -> Result<PathBuf, InstallHookError>;

    /// Whether this harness runs atuin's hooks here, as any atuin's [`Self::install_hooks`]
    /// wrote them.
    #[allow(async_fn_in_trait)]
    async fn has_hooks(&self) -> Result<bool, InstallHookError>;

    /// Take out what [`Self::install_hooks`] wrote, leaving the rest of the harness's config as
    /// it is. `false` when there was nothing to take out.
    #[allow(async_fn_in_trait)]
    async fn uninstall_hooks(&self) -> Result<bool, InstallHookError>;

    /// How this harness itself reopens `target`: its own resume command, and whether that has to
    /// run from the session's directory. `Err` for a session the harness cannot reopen.
    fn resume_plan(&self, target: &ResumeTarget) -> Result<ResumePlan, ResumeError>;

    /// [`Self::resume_plan`], with the program and arguments replaced by the user's `template`
    /// when there is one (see [`ResumePlan::with_template`]). The harness's plan still decides
    /// whether the session can be resumed at all, and from where.
    fn resume(
        &self,
        target: &ResumeTarget,
        template: Option<&str>,
    ) -> Result<ResumePlan, ResumeError> {
        let plan = self.resume_plan(target)?;
        match template {
            Some(template) => plan.with_template(template, target),
            None => Ok(plan),
        }
    }

    /// Where the native record of session `id` is on this machine, `None` when it is not here:
    /// the transcript file, or for a harness that keeps its sessions in a database, the database
    /// holding it. Looks where the harness keeps sessions by default.
    #[allow(async_fn_in_trait)]
    async fn locate(&self, id: &str) -> Option<PathBuf>;

    /// Write `session` out as this harness's own transcript, where [`Self::locate`] (and the
    /// harness itself) will find it, and return where it went, and what it left out to fit, if
    /// anything: a session recorded on another machine, or whose transcript is gone, can then be
    /// resumed like any other. Never replaces a transcript already there
    /// ([`RehydrateError::AlreadyExists`]).
    #[allow(async_fn_in_trait)]
    async fn rehydrate(&self, session: &RehydrateSession) -> Result<Rehydrated, RehydrateError>;
}

/// Whether atuin's own file is at `path`.
async fn own_file_exists(path: &Path) -> Result<bool, InstallHookError> {
    Ok(tokio::fs::try_exists(path).await?)
}

/// Remove atuin's own file at `path`, `false` when there was none.
async fn remove_own_file(path: &Path) -> Result<bool, InstallHookError> {
    match tokio::fs::remove_file(path).await {
        Ok(()) => Ok(true),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(err.into()),
    }
}

#[enum_dispatch(Harness, SessionSync, ObservableExt, InstallMcp)]
#[derive(Debug, Clone, Copy)]
pub enum AnyHarness {
    Amp(Amp),
    ClaudeCode(Ccode),
    Codex(Codex),
    Copilot(Copilot),
    Grok(Grok),
    Hermes(Hermes),
    Kiro(Kiro),
    Opencode(Opencode),
    Pi(Pi),
    Vibe(Vibe),
}

#[derive(Debug, thiserror::Error)]
pub enum HarnessLookupError {
    #[error("unknown harness. known harnesses: {0:?}")]
    Unknown(Vec<&'static str>),
}

/// Error returned when installing a harness's hooks fails.
#[derive(Debug, thiserror::Error)]
pub enum InstallHookError {
    #[error("unexpected io error")]
    Io(
        #[from]
        #[source]
        std::io::Error,
    ),

    #[error(transparent)]
    Executable(#[from] crate::atuin_exe::LaunchPathError),

    #[error("could not parse the existing config as JSON")]
    Json(
        #[from]
        #[source]
        serde_json::Error,
    ),

    #[error("the config has an unexpected shape: {0}")]
    Malformed(&'static str),

    #[error("the atuin executable path is not valid UTF-8: {}", .0.display())]
    NonUtf8Executable(PathBuf),

    #[error("could not shell-quote the atuin executable path {}", .path.display())]
    UnquotableExecutable {
        path: PathBuf,
        #[source]
        source: shlex::QuoteError,
    },

    #[error("hook already installed")]
    AlreadyInstalled,

    #[error("this agent offers no hooks for atuin to record its commands with")]
    Unsupported,
}

/// The line of a TypeScript plugin that names atuin, for [`plugin_with_executable`].
const TS_ATUIN_LINE: &str = r#"const ATUIN = "atuin";"#;

/// `plugin` with `executable` in place of the `"atuin"` on its one `line`.
///
/// The path goes in as a JSON string, which is also a string literal in JavaScript, TypeScript and
/// Python.
///
/// # Panics
///
/// If `plugin` lacks `line`, or `line` lacks `"atuin"`: the plugin would run whatever `atuin` the
/// harness finds on its `PATH`.
fn plugin_with_executable(
    plugin: &str,
    line: &str,
    executable: &Path,
) -> Result<String, InstallHookError> {
    const ATUIN: &str = r#""atuin""#;

    let path = executable
        .to_str()
        .ok_or_else(|| InstallHookError::NonUtf8Executable(executable.to_owned()))?;
    let (before, after) =
        plugin.split_once(line).expect("the plugin has the line that names atuin");
    let (line_before, line_after) = line.split_once(ATUIN).expect("the line names atuin");

    let mut named = Vec::with_capacity(plugin.len() + path.len());
    named.extend_from_slice(before.as_bytes());
    named.extend_from_slice(line_before.as_bytes());
    serde_json::to_writer(&mut named, path).expect("a string is JSON");
    named.extend_from_slice(line_after.as_bytes());
    named.extend_from_slice(after.as_bytes());
    Ok(String::from_utf8(named).expect("JSON is UTF-8"))
}

impl AnyHarness {
    /// Try to create a [`Self`] object from the given name.
    pub fn from_name(name: &str) -> Result<Self, HarnessLookupError> {
        Self::all()
            .iter()
            .find(|harness| {
                std::iter::once(harness.name())
                    .chain(harness.alias_names().iter().copied())
                    .any(|candidate| candidate == name)
            })
            .copied()
            .ok_or_else(|| {
                HarnessLookupError::Unknown(Self::all().iter().map(Harness::name).collect())
            })
    }

    /// Every known harness.
    #[must_use]
    pub fn all() -> &'static [Self] {
        &[
            Self::Amp(Amp),
            Self::ClaudeCode(Ccode),
            Self::Codex(Codex),
            Self::Copilot(Copilot),
            Self::Grok(Grok),
            Self::Hermes(Hermes),
            Self::Kiro(Kiro),
            Self::Opencode(Opencode),
            Self::Pi(Pi::PI),
            Self::Pi(Pi::OMP),
            Self::Vibe(Vibe),
        ]
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::num::NonZeroUsize;

    use rstest::rstest;

    use super::*;
    use crate::env::MockEnv;
    use crate::harnesstools::session::any::SessionsExt as _;

    /// Each agent's hooks are found once installed, and not once taken out. Hermes's need its CLI
    /// and opencode's its version, so their own tests have them.
    #[rstest]
    #[case("amp")]
    #[case("claude-code")]
    #[case("codex")]
    #[case("copilot")]
    #[case("grok")]
    #[case("kiro")]
    #[case("pi")]
    #[case("omp")]
    #[tokio::test]
    async fn hooks_come_out_as_they_went_in(#[case] name: &str) {
        let home = tempfile::tempdir().unwrap();
        let env = MockEnv::install();
        env.set("HOME", home.path());
        let harness = AnyHarness::from_name(name).unwrap();

        assert!(!harness.has_hooks().await.unwrap());
        harness.install_hooks().await.unwrap();
        assert!(harness.has_hooks().await.unwrap());
        assert!(harness.uninstall_hooks().await.unwrap());
        assert!(!harness.has_hooks().await.unwrap());
        assert!(!harness.uninstall_hooks().await.unwrap());
    }

    /// Each TypeScript plugin has the one line that installing names atuin's path on.
    #[rstest]
    #[case::amp(include_str!("../../res/harnesstools/amp/atuin.ts"))]
    #[case::opencode_1(include_str!("../../res/harnesstools/opencode/atuin.ts"))]
    #[case::opencode_2(include_str!("../../res/harnesstools/opencode/atuin.v2.ts"))]
    #[case::pi(include_str!("../../res/harnesstools/pi/atuin.ts"))]
    fn each_plugin_names_atuin_once(#[case] plugin: &str) {
        assert_eq!(plugin.matches(TS_ATUIN_LINE).count(), 1);
    }

    #[rstest]
    #[case::plain("/opt/atuin", r#"const ATUIN = "/opt/atuin";"#)]
    #[case::quotes(r#"/opt/"q"\atuin"#, r#"const ATUIN = "/opt/\"q\"\\atuin";"#)]
    fn plugin_with_executable_names_the_path_as_a_string(
        #[case] executable: &str,
        #[case] line: &str,
    ) {
        let plugin = format!("before();\n{TS_ATUIN_LINE}\nspawn(ATUIN);\n");
        let named = plugin_with_executable(&plugin, TS_ATUIN_LINE, Path::new(executable)).unwrap();
        assert_eq!(named, format!("before();\n{line}\nspawn(ATUIN);\n"));
    }

    #[rstest]
    #[should_panic(expected = "the plugin has the line that names atuin")]
    fn plugin_with_executable_refuses_a_plugin_without_the_line() {
        let _ = plugin_with_executable("spawn(\"atuin\");", TS_ATUIN_LINE, Path::new("/opt/atuin"));
    }

    /// Each harness declares exactly the variables atuin reads to find its sessions and config:
    /// a launchd agent inherits only the declared ones, so one left out would be read unset.
    #[rstest]
    fn each_harness_declares_the_variables_that_find_it() {
        let pool = BlockingPool::new(NonZeroUsize::MIN);
        for harness in AnyHarness::all() {
            let home = tempfile::tempdir().unwrap();
            let env = MockEnv::install();
            env.set("HOME", home.path());

            let _ = harness.mcp_config_path();
            let _ = harness.sessions(&pool).map(|sessions| sessions.last_active());

            let mut read = env.read();
            // Every harness's default home is under it, and the daemon always has it.
            read.remove("HOME");
            let declared: BTreeSet<_> = harness.home_vars().iter().map(|&v| v.to_owned()).collect();
            assert_eq!(read, declared, "{}", harness.name());
        }
    }
}
