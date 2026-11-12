//! Hermes Agent, Nous Research's `hermes`: reading its sessions, resuming them, and writing them.
//!
//! Hermes keeps every session in one SQLite database per home, `state.db` (see [`session`] for
//! the reader and `store` for how it opens the file). Only its gateway writes the store, so atuin
//! writes a session through its plugin's `hermes atuin import`, as `rehydrate` describes. That
//! session is a restore, a fork or a continuation. An import only adds new sessions: a copy here
//! is never caught up or switched, and sync forks instead. What atuin knows of the store was
//! verified against Hermes v0.23.0 and its main branch at d70cbc5205.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use super::rehydrate::{RehydrateError, RehydrateSession, Rehydrated};
use super::resume::{self, CwdRequirement, ResumeError, ResumePlan, ResumeTarget};
use super::{Harness, InstallHookError};
use crate::dirs::home_dir;
use crate::env::var_nonempty;
use crate::path::{AbsPathBuf, PathBufExt as _, ShellExpanded};

pub mod cli;
pub mod home;
pub mod hooks;
pub mod mcp;
pub(crate) mod rehydrate;
pub mod session;
pub(crate) mod store;

/// The Hermes home: `$HERMES_HOME` (with `~` and `$VAR` expanded, as Hermes expands it), else
/// `~/.hermes` (`%LOCALAPPDATA%\hermes` on Windows). Profiles live below it, in
/// `profiles/<name>/`.
pub(crate) fn hermes_home() -> PathBuf {
    home_from(
        var_nonempty("HERMES_HOME").as_deref(),
        var_nonempty("HERMES_DATA_DIR_SUFFIX").as_deref(),
        &home_dir(),
    )
}

/// [`hermes_home`] for the variables `var` (`HERMES_HOME`) and `suffix`
/// (`HERMES_DATA_DIR_SUFFIX`, which names the default home `~/.hermes<suffix>`), with `home` the
/// user's home directory. Hermes's own rule is `_platform_default_hermes_home` in
/// `hermes_constants.py`.
fn home_from(var: Option<&OsStr>, suffix: Option<&OsStr>, home: &Path) -> PathBuf {
    if let Some(var) = var {
        let expanded = var.to_str().and_then(|raw| ShellExpanded::expand(raw).ok());
        return expanded.map_or_else(|| PathBuf::from(var), PathBuf::from);
    }
    let (base, name) = if cfg!(windows) {
        let local = var_nonempty("LOCALAPPDATA")
            .map_or_else(|| home.join("AppData").joined("Local"), PathBuf::from);
        (local, "hermes")
    } else {
        (home.to_path_buf(), ".hermes")
    };
    let mut name = std::ffi::OsString::from(name);
    if let Some(suffix) = suffix {
        name.push(suffix);
    }
    base.joined(name)
}

/// The variables of `environment`, this shell's, that an atuin Hermes runs needs to read the same
/// config and data: the `ATUIN_` settings that say where something is, by name, those ending
/// `_HOME`, `_DIR` or `_PATH`, such as `ATUIN_HOME`, `ATUIN_CONFIG_DIR` or `ATUIN_DB_PATH`.
///
/// Hermes runs atuin from its gateway, without the shell's variables: as an MCP server it passes
/// on only a few of its own, and a gateway that `launchd` or `systemd` starts has hardly any. So
/// installs write these into what Hermes keeps, the MCP server's entry in `config.yaml` and the
/// hook plugin. Only these, as a list of what to keep rather than of what to leave out: people
/// share those files, and an `ATUIN_` variable can hold a secret, such as `ATUIN_AI__API_TOKEN`.
/// A setting left out is read from atuin's config instead.
///
/// A relative `ATUIN_CONFIG_DIR` is made absolute against this process's directory, where atuin
/// reads it from (`dirs::config_path`): Hermes runs atuin from a directory of its own. The other
/// relative values stay as they are, since atuin reads them the same anywhere: `ATUIN_HOME` must
/// be absolute or is ignored, and a setting's path is relative to the config directory.
pub fn atuin_env(
    environment: impl IntoIterator<Item = (String, String)>,
) -> BTreeMap<String, String> {
    environment
        .into_iter()
        .filter(|(name, _)| {
            name.starts_with("ATUIN_")
                && ["_HOME", "_DIR", "_PATH"].iter().any(|suffix| name.ends_with(suffix))
        })
        .map(|(name, value)| {
            let value = if name == "ATUIN_CONFIG_DIR" {
                absolute(value)
            } else {
                value
            };
            (name, value)
        })
        .collect()
}

/// `path` made absolute against this process's directory, or as it was when it can't be: the
/// directory is gone, or isn't Unicode.
fn absolute(path: String) -> String {
    AbsPathBuf::from_current_dir(&path)
        .ok()
        .and_then(|absolute| PathBuf::from(absolute).into_os_string().into_string().ok())
        .unwrap_or(path)
}

/// The home installs go to: the one a bare `hermes` uses (see [`home::InstallHome::resolve`]).
pub(crate) fn install_home() -> home::InstallHome {
    home::InstallHome::resolve(&hermes_home())
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Hermes;

impl Harness for Hermes {
    fn name(&self) -> &'static str {
        "hermes"
    }

    fn alias_names(&self) -> &'static [&'static str] {
        &["hermes-agent"]
    }

    fn display_name(&self) -> &'static str {
        "Hermes Agent"
    }

    fn program(&self) -> &'static str {
        "hermes"
    }

    /// `LOCALAPPDATA` holds the default home on Windows only.
    fn home_vars(&self) -> &'static [&'static str] {
        if cfg!(windows) {
            &["HERMES_HOME", "HERMES_DATA_DIR_SUFFIX", "LOCALAPPDATA"]
        } else {
            &["HERMES_HOME", "HERMES_DATA_DIR_SUFFIX"]
        }
    }

    /// Writes atuin's plugin to the active home's `plugins/atuin/` and enables it (see [`hooks`]).
    async fn install_hooks(&self) -> Result<PathBuf, InstallHookError> {
        let atuin = super::mcp::launch_path(std::env::current_exe()?);
        let env = atuin_env(crate::env::vars_utf8());
        hooks::install(&install_home(), &atuin, &env).await
    }

    /// Yes: Hermes's import takes every row it's given as live, so a turn `/undo` dropped would
    /// come back beside the one that replaced it.
    fn writes_one_branch(&self) -> bool {
        true
    }

    /// Yes: a fork's `model_config._branched_from` is the only link Hermes's reader reads for it.
    fn restores_fork_links(&self) -> bool {
        true
    }

    /// `hermes --resume <id>`, from anywhere: Hermes changes into the session's own directory
    /// itself. A session found here resumes under the profile whose store holds it (`-p <name>`,
    /// `-p default` for the home's own), the only store `hermes` then reads: a bare `hermes`
    /// follows the profile `hermes profile use` picked.
    fn resume_plan(&self, target: &ResumeTarget) -> Result<ResumePlan, ResumeError> {
        let profile = target
            .native_path
            .as_deref()
            .map(|db| store::profile_of(db).unwrap_or_else(|| home::DEFAULT.to_owned()));
        let args = profile
            .into_iter()
            .flat_map(|profile| ["-p".to_owned(), profile])
            .chain(["--resume".to_owned(), target.id.clone()]);
        Ok(resume::plan(target, CwdRequirement::None, self.program(), args))
    }

    async fn locate(&self, id: &str) -> Option<PathBuf> {
        session::locate(&hermes_home(), id).await
    }

    /// Into the store that a bare `hermes` uses, through `hermes atuin import`, as `rehydrate`
    /// describes.
    async fn rehydrate(&self, session: &RehydrateSession) -> Result<Rehydrated, RehydrateError> {
        rehydrate::rehydrate(&install_home(), session).await
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[rstest]
    fn resumes_by_id_from_anywhere() {
        let target = ResumeTarget::new("local-8bae").with_cwd("/work");
        let plan = Hermes.resume_plan(&target).unwrap();
        assert_eq!(plan.program, "hermes");
        assert_eq!(plan.args, ["--resume", "local-8bae"]);
        assert_eq!(plan.cwd_requirement, CwdRequirement::None);
    }

    #[rstest]
    #[case::home("/u/.hermes/state.db", &["-p", "default", "--resume", "s1"])]
    #[case::profile("/u/.hermes/profiles/work/state.db", &["-p", "work", "--resume", "s1"])]
    #[case::not_a_profile("/u/.hermes/other/work/state.db", &["-p", "default", "--resume", "s1"])]
    fn a_profiles_session_resumes_under_its_profile(#[case] db: &str, #[case] want: &[&str]) {
        let target = ResumeTarget::new("s1").with_native_path(db);
        assert_eq!(Hermes.resume_plan(&target).unwrap().args, want);
    }

    /// Only where atuin's config and data are: never a secret, shell state, or anything else.
    #[rstest]
    #[case::home("ATUIN_HOME", true)]
    #[case::config_dir("ATUIN_CONFIG_DIR", true)]
    #[case::db_path("ATUIN_DB_PATH", true)]
    #[case::nested_path("ATUIN_DAEMON__SOCKET_PATH", true)]
    #[case::key_path("ATUIN_KEY_PATH", true)]
    #[case::ai_token("ATUIN_AI__API_TOKEN", false)]
    #[case::extra_header("ATUIN_EXTRA_HEADERS__CF_ACCESS_CLIENT_SECRET", false)]
    #[case::password("ATUIN_PASSWORD", false)]
    #[case::session("ATUIN_SESSION", false)]
    #[case::other_setting("ATUIN_SEARCH_MODE", false)]
    #[case::not_atuin("HISTDIR", false)]
    fn atuin_env_holds_only_locations(#[case] name: &str, #[case] kept: bool) {
        let env = atuin_env([(name.to_owned(), "x".to_owned())]);
        assert_eq!(env.contains_key(name), kept);
    }

    /// A relative `ATUIN_CONFIG_DIR` names the directory where it was set, not where Hermes runs;
    /// a relative setting's path is the config directory's, wherever atuin runs.
    #[rstest]
    fn a_relative_config_dir_is_made_absolute() {
        let env = atuin_env([
            ("ATUIN_CONFIG_DIR".to_owned(), "config".to_owned()),
            ("ATUIN_DB_PATH".to_owned(), "history.db".to_owned()),
            ("ATUIN_HOME".to_owned(), "/srv/atuin".to_owned()),
        ]);
        let here = std::env::current_dir().unwrap();
        assert_eq!(env["ATUIN_CONFIG_DIR"], here.join("config").to_str().unwrap());
        assert_eq!(env["ATUIN_DB_PATH"], "history.db");
        assert_eq!(env["ATUIN_HOME"], "/srv/atuin");
    }

    #[rstest]
    fn is_found_by_its_names() {
        for name in ["hermes", "hermes-agent"] {
            let harness = super::super::AnyHarness::from_name(name).unwrap();
            assert_eq!(harness.name(), "hermes", "{name}");
        }
    }

    #[cfg(unix)]
    #[rstest]
    #[case::unset(None, None, "/home/u/.hermes")]
    #[case::set(Some("/srv/hermes"), None, "/srv/hermes")]
    #[case::tilde(Some("~/h"), None, "/home/u/h")]
    #[case::variable(Some("$HERMES_TEST_ROOT/h"), None, "/data/h")]
    #[case::suffix(None, Some("-dev"), "/home/u/.hermes-dev")]
    #[case::suffix_ignored_when_set(Some("/srv/hermes"), Some("-dev"), "/srv/hermes")]
    fn home_follows_hermes_rule(
        #[case] var: Option<&str>,
        #[case] suffix: Option<&str>,
        #[case] want: &str,
    ) {
        let env = crate::env::mock::MockEnv::install();
        env.set("HOME", "/home/u");
        env.set("HERMES_TEST_ROOT", "/data");
        let got = home_from(var.map(OsStr::new), suffix.map(OsStr::new), Path::new("/home/u"));
        assert_eq!(got, PathBuf::from(want));
    }
}
