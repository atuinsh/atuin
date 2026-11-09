//! Register atuin's MCP server (`atuin mcp`) with a harness, so its agent can search the user's
//! history, command output and agent sessions.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use enum_dispatch::enum_dispatch;
use serde_json::{Value, json};

use super::Harness;
use super::amp::Amp;
use super::ccode::Ccode;
use super::codex::{self, Codex};
use super::copilot::{self, Copilot};
use super::grok::{self, Grok};
use super::kiro::{self, Kiro};
use super::opencode::{self, Opencode};
use super::pi::{Flavour, Pi};
use crate::dirs::home_dir;
use crate::env::var_nonempty;
use crate::executable::resolve_in_path;
use crate::path::{AbsPathBuf, PathBufExt as _};

#[derive(Debug, thiserror::Error)]
pub enum InstallMcpError {
    #[error("unexpected io error")]
    Io(
        #[from]
        #[source]
        std::io::Error,
    ),

    #[error("could not parse {} as JSON", .0.display())]
    Json(PathBuf, #[source] serde_json::Error),

    #[error("{} has an unexpected shape: `{}` is not an object", .0.display(), .1)]
    Malformed(PathBuf, &'static str),

    #[error("could not save the config")]
    Replace(
        #[from]
        #[source]
        crate::fs::ReplaceError,
    ),

    #[error("`{0}` is not on your PATH; install it first")]
    NotFound(&'static str),

    #[error("`{0}` failed")]
    Failed(String),

    #[error("the atuin executable path is not valid UTF-8: {}", .0.display())]
    NonUtf8Executable(PathBuf),
}

/// Registers atuin's MCP server (`atuin mcp`) with a harness at user scope.
#[enum_dispatch]
pub trait InstallMcp: Harness {
    /// The user config that registration writes. Either the harness's own `mcp add` writes it, or
    /// atuin does.
    fn mcp_config_path(&self) -> PathBuf;

    /// Whether [`Self::mcp_config_path`] exists. This detects agents that have a config without
    /// starting their executables. It does not check whether the atuin server is registered.
    fn has_mcp_config(&self) -> bool {
        self.mcp_config_path().is_file()
    }

    /// The program that [`Self::install_mcp`] runs to register the server. That program has to be
    /// on `PATH`. By default, it is the harness's own [`Harness::program`]. It is `None` for a
    /// harness whose config atuin writes itself.
    fn mcp_program(&self) -> Option<&'static str> {
        Some(self.program())
    }

    /// Registers the server. Any `atuin` server that is already registered is replaced, so that a
    /// rerun picks up a binary that has moved.
    #[allow(async_fn_in_trait)]
    async fn install_mcp(&self) -> Result<(), InstallMcpError>;
}

impl InstallMcp for Amp {
    fn mcp_config_path(&self) -> PathBuf {
        home_dir().joined(".config").joined("amp").joined("settings.json")
    }

    async fn install_mcp(&self) -> Result<(), InstallMcpError> {
        let atuin = &executable()?;
        // `amp mcp add` refuses a name that's taken.
        remove(self.program(), &["mcp", "remove", "atuin"]).await;
        run(self.program(), &["mcp", "add", "atuin", "--", atuin, "mcp"]).await
    }
}

impl InstallMcp for Ccode {
    fn mcp_config_path(&self) -> PathBuf {
        var_nonempty("CLAUDE_CONFIG_DIR").map_or_else(
            || home_dir().joined(".claude.json"),
            |dir| PathBuf::from(dir).joined(".claude.json"),
        )
    }

    async fn install_mcp(&self) -> Result<(), InstallMcpError> {
        let atuin = &executable()?;
        // `claude mcp add` refuses a name that's taken.
        remove(self.program(), &["mcp", "remove", "--scope", "user", "atuin"]).await;
        run(self.program(), &["mcp", "add", "--scope", "user", "atuin", "--", atuin, "mcp"]).await
    }
}

impl InstallMcp for Codex {
    fn mcp_config_path(&self) -> PathBuf {
        codex::home().joined("config.toml")
    }

    async fn install_mcp(&self) -> Result<(), InstallMcpError> {
        let atuin = &executable()?;
        run(self.program(), &["mcp", "add", "atuin", "--", atuin, "mcp"]).await
    }
}

impl InstallMcp for Copilot {
    fn mcp_config_path(&self) -> PathBuf {
        copilot::copilot_home().joined("mcp-config.json")
    }

    /// `copilot mcp add` writes `~/.copilot/mcp-config.json`. It replaces any server that has the
    /// same name.
    async fn install_mcp(&self) -> Result<(), InstallMcpError> {
        let atuin = &executable()?;
        run(self.program(), &["mcp", "add", "atuin", "--", atuin, "mcp"]).await
    }
}

impl InstallMcp for Grok {
    fn mcp_config_path(&self) -> PathBuf {
        grok::home().joined("config.toml")
    }

    async fn install_mcp(&self) -> Result<(), InstallMcpError> {
        let atuin = &executable()?;
        run(self.program(), &["mcp", "add", "atuin", "--", atuin, "mcp"]).await
    }
}

impl InstallMcp for Kiro {
    fn mcp_config_path(&self) -> PathBuf {
        kiro_config_path(&kiro::kiro_home())
    }

    /// Atuin writes Kiro's config itself instead of running `kiro-cli`. The binary has a different
    /// name on each platform, and the config file is plain JSON.
    fn mcp_program(&self) -> Option<&'static str> {
        None
    }

    async fn install_mcp(&self) -> Result<(), InstallMcpError> {
        install_kiro(&self.mcp_config_path(), &executable()?).await
    }
}

impl InstallMcp for Opencode {
    fn mcp_config_path(&self) -> PathBuf {
        opencode::config_dir().joined("opencode.json")
    }

    /// Atuin writes opencode's config itself instead of running `opencode mcp add`. In opencode
    /// 1.x, that command only prompts. In 2.0, it writes `mcp.servers`, which 2.0 doesn't load.
    /// Both versions load `mcp.<name>`.
    fn mcp_program(&self) -> Option<&'static str> {
        None
    }

    async fn install_mcp(&self) -> Result<(), InstallMcpError> {
        install_opencode(&self.mcp_config_path(), &executable()?).await
    }
}

impl InstallMcp for Pi {
    fn mcp_config_path(&self) -> PathBuf {
        self.flavour().agent_dir().joined("mcp.json")
    }

    /// omp has no `mcp add` command, so atuin writes [`Self::mcp_config_path`] itself. omp's own
    /// `/mcp add` writes the same file.
    fn mcp_program(&self) -> Option<&'static str> {
        match self.flavour() {
            Flavour::Pi => Some(self.program()),
            Flavour::Omp => None,
        }
    }

    async fn install_mcp(&self) -> Result<(), InstallMcpError> {
        let atuin = &executable()?;
        match self.mcp_program() {
            Some(program) => run(program, &["mcp", "add", "atuin", "--", atuin, "mcp"]).await,
            None => {
                let server = json!({"command": atuin, "args": ["mcp"]});
                add_to_config(&self.mcp_config_path(), json!({}), "mcpServers", server).await
            }
        }
    }
}

/// Run `program` with `args` to remove a server, whether or not there is one to remove.
async fn remove(program: &str, args: &[&str]) {
    let Some(program) = resolve_in_path(Path::new(program)) else {
        return;
    };
    let _ = tokio::process::Command::new(&*program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await;
}

/// The atuin for a harness to launch. Harnesses don't necessarily have the user's `PATH`, so it's
/// an absolute path: see [`launch_path`].
fn executable() -> Result<String, InstallMcpError> {
    launch_path(std::env::current_exe()?)
        .into_os_string()
        .into_string()
        .map_err(|path| InstallMcpError::NonUtf8Executable(path.into()))
}

/// The first atuin in an absolute `PATH` entry where that is `exe`, since a package manager's
/// entry there (a Homebrew or Nix profile symlink) survives upgrades where the resolved path
/// doesn't, and `exe` otherwise.
fn launch_path(exe: PathBuf) -> PathBuf {
    let on_path = crate::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .filter(|dir| dir.is_absolute())
            .map(|dir| dir.joined(format!("atuin{}", std::env::consts::EXE_SUFFIX)))
            .find(|candidate| candidate.is_file())
    });
    match on_path {
        Some(on_path)
            if AbsPathBuf::canonicalize(&on_path).ok() == AbsPathBuf::canonicalize(&exe).ok() =>
        {
            on_path
        }
        _ => exe,
    }
}

/// Runs `program` with `args` and lets it report what it did. The program runs from the path that
/// [`resolve_in_path`] finds, so that what runs is what the `PATH` check found. For example, on
/// Windows, `claude` runs as `claude.cmd`.
async fn run(program: &'static str, args: &[&str]) -> Result<(), InstallMcpError> {
    let found = resolve_in_path(Path::new(program)).ok_or(InstallMcpError::NotFound(program))?;
    let status =
        tokio::process::Command::new(&*found).args(args).stdin(Stdio::null()).status().await?;
    if !status.success() {
        return Err(InstallMcpError::Failed(format!("{program} {}", args.join(" "))));
    }
    Ok(())
}

/// Add the server to the opencode config at `path`, keeping the rest of it.
async fn install_opencode(path: &Path, atuin: &str) -> Result<(), InstallMcpError> {
    let server = json!({"type": "local", "command": [atuin, "mcp"], "enabled": true});
    let new = json!({"$schema": "https://opencode.ai/config.json"});
    add_to_config(path, new, "mcp", server).await
}

/// Kiro's user-level MCP config under its home (`$KIRO_HOME`, else `~/.kiro`): the file
/// `kiro-cli mcp add --scope global` writes, which every session loads.
fn kiro_config_path(kiro_home: &Path) -> PathBuf {
    kiro_home.join("settings").joined("mcp.json")
}

/// Add the server to the Kiro config at `path`, keeping the rest of it, as `kiro-cli mcp add
/// --scope global` would (Kiro CLI 2.36.0). Not through `kiro-cli` itself: its binary is named
/// differently per platform, and the file is plain JSON.
async fn install_kiro(path: &Path, atuin: &str) -> Result<(), InstallMcpError> {
    let server = json!({"command": atuin, "args": ["mcp"], "env": {}});
    add_to_config(path, json!({}), "mcpServers", server).await
}

/// Add `server` as `atuin` to the `key` object of the JSON config at `path`, `new` when there is
/// none yet.
async fn add_to_config(
    path: &Path,
    new: Value,
    key: &'static str,
    server: Value,
) -> Result<(), InstallMcpError> {
    // A config linked from a dotfiles repo is saved there, keeping the link.
    let path =
        &AbsPathBuf::canonicalize_async(path).await.map_or_else(|_| path.to_owned(), PathBuf::from);
    let read = match tokio::fs::read(path).await {
        Ok(read) => Some(read),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
        Err(err) => return Err(err.into()),
    };
    let mut root: Value = match &read {
        Some(read) => serde_json::from_slice(read)
            .map_err(|err| InstallMcpError::Json(path.to_owned(), err))?,
        None => new,
    };
    root.as_object_mut()
        .and_then(|root| root.entry(key).or_insert_with(|| json!({})).as_object_mut())
        .ok_or_else(|| InstallMcpError::Malformed(path.to_owned(), key))?
        .insert("atuin".to_owned(), server);

    let text = serde_json::to_string_pretty(&root)
        .map_err(|err| InstallMcpError::Json(path.to_owned(), err))?
        + "\n";
    // Never written in place, so a failed write can't lose the rest of the config.
    match &read {
        // Only over the config read, not one opencode rewrote meanwhile.
        Some(read) => match crate::fs::replace(path, text.as_bytes(), |now| now == read) {
            Err(err) if !err.replaced() => return Err(err.into()),
            _ => {}
        },
        None => {
            if let Some(parent) = path.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }
            crate::fs::write_new(path, text.as_bytes())?;
        }
    }
    crate::userio::log::info!("mcp-server-added", path = path.display().to_string());
    Ok(())
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;
    use crate::env::mock::MockEnv;
    use crate::harnesstools::AnyHarness;

    #[rstest::fixture]
    fn agent_home() -> (tempfile::TempDir, MockEnv) {
        let dir = tempfile::tempdir().unwrap();
        let env = MockEnv::install();
        env.set("HOME", dir.path());
        (dir, env)
    }

    #[rstest]
    #[case("claude-code", ".claude.json")]
    #[case("codex", ".codex/config.toml")]
    #[case("copilot", ".copilot/mcp-config.json")]
    #[case("grok", ".grok/config.toml")]
    #[case("kiro", ".kiro/settings/mcp.json")]
    #[case("opencode", ".config/opencode/opencode.json")]
    #[case("pi", ".pi/agent/mcp.json")]
    #[case("omp", ".omp/agent/mcp.json")]
    fn detects_existing_config_files(
        agent_home: (tempfile::TempDir, MockEnv),
        #[case] name: &str,
        #[case] relative: &str,
    ) {
        let (dir, _env) = agent_home;
        let harness = AnyHarness::from_name(name).unwrap();
        let path = dir.path().join(relative);
        assert_eq!(harness.mcp_config_path(), path);
        assert!(!harness.has_mcp_config());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        // Detection needs a file, not just an agent directory.
        assert!(!harness.has_mcp_config());
        std::fs::write(&path, "").unwrap();
        assert!(harness.has_mcp_config());
    }

    #[rstest]
    #[case("claude-code", "CLAUDE_CONFIG_DIR", ".claude.json")]
    #[case("codex", "CODEX_HOME", "config.toml")]
    #[case("copilot", "COPILOT_HOME", "mcp-config.json")]
    #[case("grok", "GROK_HOME", "config.toml")]
    #[case("kiro", "KIRO_HOME", "settings/mcp.json")]
    #[case("opencode", "XDG_CONFIG_HOME", "opencode/opencode.json")]
    #[case("pi", "PI_CODING_AGENT_DIR", "mcp.json")]
    #[case("omp", "PI_CODING_AGENT_DIR", "mcp.json")]
    fn detection_respects_config_overrides(
        agent_home: (tempfile::TempDir, MockEnv),
        #[case] name: &str,
        #[case] variable: &str,
        #[case] relative: &str,
    ) {
        let (dir, env) = agent_home;
        let custom = dir.path().join("custom");
        env.set(variable, &custom);
        let harness = AnyHarness::from_name(name).unwrap();
        let path = custom.join(relative);
        assert_eq!(harness.mcp_config_path(), path);
        assert!(!harness.has_mcp_config());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "").unwrap();
        assert!(harness.has_mcp_config());
    }

    /// A Homebrew-style install: `bin/atuin` links to the real executable.
    #[cfg(unix)]
    #[rstest]
    #[case::absolute_entry(false, "bin/atuin")]
    #[case::relative_entry(true, "real/atuin")]
    fn launch_path_takes_atuin_only_from_absolute_path_entries(
        #[case] relative: bool,
        #[case] want: &str,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let (real, bin) = (dir.path().join("real"), dir.path().join("bin"));
        std::fs::create_dir_all(&real).unwrap();
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(real.join("atuin"), "").unwrap();
        std::os::unix::fs::symlink(real.join("atuin"), bin.join("atuin")).unwrap();

        // The same `bin`, relative to the current directory: it finds the link from here only.
        let entry = if relative {
            let cwd = std::env::current_dir().unwrap();
            let up: PathBuf = std::iter::repeat_n("..", cwd.components().count() - 1).collect();
            up.joined(bin.strip_prefix("/").unwrap())
        } else {
            bin
        };
        assert!(entry.join("atuin").is_file());
        let env = MockEnv::install();
        env.set("PATH", entry);

        assert_eq!(launch_path(real.join("atuin")), dir.path().join(want));
    }

    #[cfg(unix)]
    #[rstest]
    #[tokio::test]
    async fn opencode_keeps_a_linked_config_linked() {
        let dir = tempfile::tempdir().unwrap();
        let (target, link) = (dir.path().join("dotfiles.json"), dir.path().join("opencode.json"));
        std::fs::write(&target, "{}").unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();

        install_opencode(&link, "/bin/atuin").await.unwrap();

        assert!(link.symlink_metadata().unwrap().is_symlink());
        let config: Value =
            serde_json::from_str(&std::fs::read_to_string(&target).unwrap()).unwrap();
        assert_eq!(config["mcp"]["atuin"]["command"], json!(["/bin/atuin", "mcp"]));
    }

    #[rstest]
    #[tokio::test]
    async fn opencode_gets_a_new_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("opencode").joined("opencode.json");

        install_opencode(&path, "/bin/atuin").await.unwrap();

        let config: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(config["mcp"]["atuin"]["command"], json!(["/bin/atuin", "mcp"]));
    }

    /// The file `kiro-cli mcp add --scope global` writes, exactly.
    #[rstest]
    #[tokio::test]
    async fn kiro_gets_a_new_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = kiro_config_path(dir.path());

        install_kiro(&path, "/bin/atuin").await.unwrap();

        let config: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            config,
            json!({"mcpServers": {"atuin": {"command": "/bin/atuin", "args": ["mcp"], "env": {}}}})
        );
    }

    #[rstest]
    #[tokio::test]
    async fn kiro_keeps_the_rest_of_its_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mcp.json");
        std::fs::write(&path, r#"{"mcpServers": {"other": {"url": "https://x.test/mcp"}}}"#)
            .unwrap();

        install_kiro(&path, "/bin/atuin").await.unwrap();
        install_kiro(&path, "/opt/atuin").await.unwrap();

        let config: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            config,
            json!({
                "mcpServers": {
                    "other": {"url": "https://x.test/mcp"},
                    "atuin": {"command": "/opt/atuin", "args": ["mcp"], "env": {}},
                },
            })
        );
    }

    #[rstest]
    #[tokio::test]
    async fn opencode_keeps_the_rest_of_its_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("opencode.json");
        std::fs::write(&path, r#"{"theme": "x", "mcp": {"other": {"type": "remote"}}}"#).unwrap();

        install_opencode(&path, "/bin/atuin").await.unwrap();
        install_opencode(&path, "/opt/atuin").await.unwrap();

        let config: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            config,
            json!({
                "theme": "x",
                "mcp": {
                    "other": {"type": "remote"},
                    "atuin": {"type": "local", "command": ["/opt/atuin", "mcp"], "enabled": true},
                },
            })
        );
    }
}
