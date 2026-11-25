//! Register atuin's MCP server (`atuin mcp`) with a harness, so its agent can search the user's
//! history, command output and agent sessions.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use enum_dispatch::enum_dispatch;
use serde_json::{Value, json};
use toml_edit::{DocumentMut, Item};

use super::Harness;
use super::amp::{self, Amp};
use super::ccode::Ccode;
use super::codex::{self, Codex};
use super::copilot::{self, Copilot};
use super::grok::{self, Grok};
use super::hermes::cli::{HermesCliError, HermesCommand};
use super::hermes::{self, Hermes};
use super::kiro::{self, Kiro};
use super::opencode::{self, Opencode};
use super::pi::{Flavour, Pi};
use super::vibe::{self, Vibe};
use crate::atuin_exe;
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

    #[error(transparent)]
    Executable(#[from] crate::atuin_exe::LaunchPathError),

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

    #[error("`{command}` failed: {said}")]
    FailedSaying {
        command: String,
        said: String,
    },

    #[error("the atuin executable path is not valid UTF-8: {}", .0.display())]
    NonUtf8Executable(PathBuf),

    #[error("could not parse {} as TOML", .0.display())]
    Toml(PathBuf, #[source] toml_edit::TomlError),

    #[error("could not parse {} as YAML", .0.display())]
    Yaml(PathBuf, #[source] yaml_rust2::ScanError),

    #[error("couldn't take `{key}` out of {} ({cause}): remove it yourself", .path.display())]
    RemoveByHand {
        path: PathBuf,
        key: &'static str,
        cause: HermesCliError,
    },
}

impl From<HermesCliError> for InstallMcpError {
    fn from(err: HermesCliError) -> Self {
        match err {
            HermesCliError::NotFound => Self::NotFound("hermes"),
            HermesCliError::Failed(printed) => {
                Self::Failed(format!("hermes config set: {printed}"))
            }
            HermesCliError::TimedOut => Self::Failed(format!("hermes config set: {err}")),
            HermesCliError::Io(err) => Self::Io(err),
        }
    }
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

    /// Whether an `atuin` server is registered in [`Self::mcp_config_path`].
    #[allow(async_fn_in_trait)]
    async fn has_mcp(&self) -> Result<bool, InstallMcpError>;

    /// Unregister the `atuin` server, leaving the rest of the config as it is. `false` when there
    /// was none.
    #[allow(async_fn_in_trait)]
    async fn uninstall_mcp(&self) -> Result<bool, InstallMcpError>;
}

impl InstallMcp for Amp {
    fn mcp_config_path(&self) -> PathBuf {
        amp::config_dir().joined("settings.json")
    }

    async fn install_mcp(&self) -> Result<(), InstallMcpError> {
        let atuin = executable().await?;
        // `amp mcp add` refuses a name that's taken.
        remove(self.program(), &["mcp", "remove", "atuin"]).await;
        run(self.program(), &["mcp", "add", "atuin", "--", atuin, "mcp"]).await
    }

    async fn has_mcp(&self) -> Result<bool, InstallMcpError> {
        json_has_server(&self.mcp_config_path(), "amp.mcpServers").await
    }

    async fn uninstall_mcp(&self) -> Result<bool, InstallMcpError> {
        if !self.has_mcp().await? {
            return Ok(false);
        }
        run(self.program(), &["mcp", "remove", "atuin"]).await?;
        Ok(true)
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
        let atuin = executable().await?;
        // `claude mcp add` refuses a name that's taken.
        remove(self.program(), &["mcp", "remove", "--scope", "user", "atuin"]).await;
        run(self.program(), &["mcp", "add", "--scope", "user", "atuin", "--", atuin, "mcp"]).await
    }

    async fn has_mcp(&self) -> Result<bool, InstallMcpError> {
        json_has_server(&self.mcp_config_path(), "mcpServers").await
    }

    async fn uninstall_mcp(&self) -> Result<bool, InstallMcpError> {
        if !self.has_mcp().await? {
            return Ok(false);
        }
        run(self.program(), &["mcp", "remove", "--scope", "user", "atuin"]).await?;
        Ok(true)
    }
}

impl InstallMcp for Codex {
    fn mcp_config_path(&self) -> PathBuf {
        codex::home().joined("config.toml")
    }

    async fn install_mcp(&self) -> Result<(), InstallMcpError> {
        let atuin = executable().await?;
        run(self.program(), &["mcp", "add", "atuin", "--", atuin, "mcp"]).await
    }

    async fn has_mcp(&self) -> Result<bool, InstallMcpError> {
        toml_has_server(&self.mcp_config_path()).await
    }

    async fn uninstall_mcp(&self) -> Result<bool, InstallMcpError> {
        remove_toml_server(&self.mcp_config_path()).await
    }
}

impl InstallMcp for Copilot {
    fn mcp_config_path(&self) -> PathBuf {
        copilot::copilot_home().joined("mcp-config.json")
    }

    /// `copilot mcp add` writes `~/.copilot/mcp-config.json`. It replaces any server that has the
    /// same name.
    async fn install_mcp(&self) -> Result<(), InstallMcpError> {
        let atuin = executable().await?;
        run(self.program(), &["mcp", "add", "atuin", "--", atuin, "mcp"]).await
    }

    async fn has_mcp(&self) -> Result<bool, InstallMcpError> {
        json_has_server(&self.mcp_config_path(), "mcpServers").await
    }

    async fn uninstall_mcp(&self) -> Result<bool, InstallMcpError> {
        remove_from_config(&self.mcp_config_path(), "mcpServers").await
    }
}

impl InstallMcp for Grok {
    fn mcp_config_path(&self) -> PathBuf {
        grok::home().joined("config.toml")
    }

    async fn install_mcp(&self) -> Result<(), InstallMcpError> {
        let atuin = executable().await?;
        run(self.program(), &["mcp", "add", "atuin", "--", atuin, "mcp"]).await
    }

    async fn has_mcp(&self) -> Result<bool, InstallMcpError> {
        toml_has_server(&self.mcp_config_path()).await
    }

    async fn uninstall_mcp(&self) -> Result<bool, InstallMcpError> {
        remove_toml_server(&self.mcp_config_path()).await
    }
}

impl InstallMcp for Hermes {
    fn mcp_config_path(&self) -> PathBuf {
        hermes::mcp::config_path(hermes::install_home().path())
    }

    /// Through `hermes config set` (see [`hermes::mcp::install`]), so `mcp_program` stays the
    /// default, `hermes`, with where this shell's atuin keeps its config and data in the server's
    /// `env` ([`location_env`](crate::atuin_exe::location_env)).
    async fn install_mcp(&self) -> Result<(), InstallMcpError> {
        let hermes = HermesCommand::new(&hermes::install_home());
        let env = crate::atuin_exe::location_env(crate::env::vars_utf8());
        Ok(hermes::mcp::install(&hermes, executable().await?, &env).await?)
    }

    async fn has_mcp(&self) -> Result<bool, InstallMcpError> {
        yaml_has_server(&self.mcp_config_path()).await
    }

    /// Through `hermes config unset` (see [`hermes::mcp::uninstall`]), as atuin can't write
    /// `config.yaml` itself without losing its comments. A Hermes that can't, as one not on
    /// `PATH`, or one from before `config unset`, leaves it to the user.
    async fn uninstall_mcp(&self) -> Result<bool, InstallMcpError> {
        if !self.has_mcp().await? {
            return Ok(false);
        }
        let hermes = HermesCommand::new(&hermes::install_home());
        let unset = hermes::mcp::uninstall(&hermes).await;
        unset.map_err(|cause| InstallMcpError::RemoveByHand {
            path: self.mcp_config_path(),
            key: "mcp_servers.atuin",
            cause,
        })?;
        Ok(true)
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
        install_kiro(&self.mcp_config_path(), executable().await?).await
    }

    async fn has_mcp(&self) -> Result<bool, InstallMcpError> {
        json_has_server(&self.mcp_config_path(), "mcpServers").await
    }

    async fn uninstall_mcp(&self) -> Result<bool, InstallMcpError> {
        remove_from_config(&self.mcp_config_path(), "mcpServers").await
    }
}

impl InstallMcp for Vibe {
    fn mcp_config_path(&self) -> PathBuf {
        vibe::home().joined("config.toml")
    }

    async fn install_mcp(&self) -> Result<(), InstallMcpError> {
        let atuin = executable().await?;
        // `vibe mcp add` keeps a server that is already there.
        remove(self.program(), &["mcp", "remove", "atuin"]).await;
        run(self.program(), &[
            "mcp",
            "add",
            "atuin",
            "--transport",
            "stdio",
            "--command",
            atuin,
            "--arg",
            "mcp",
        ])
        .await
    }

    /// Vibe lists its servers as an array of tables, `[[mcp_servers]]`, each named by `name`.
    async fn has_mcp(&self) -> Result<bool, InstallMcpError> {
        let Some((_, _, doc)) = read_toml(&self.mcp_config_path()).await? else {
            return Ok(false);
        };
        let servers = doc.get("mcp_servers").and_then(Item::as_array_of_tables);
        Ok(servers.is_some_and(|servers| {
            servers.iter().any(|server| server.get("name").and_then(Item::as_str) == Some("atuin"))
        }))
    }

    async fn uninstall_mcp(&self) -> Result<bool, InstallMcpError> {
        if !self.has_mcp().await? {
            return Ok(false);
        }
        run(self.program(), &["mcp", "remove", "atuin"]).await?;
        Ok(true)
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
        install_opencode(&self.mcp_config_path(), executable().await?).await
    }

    async fn has_mcp(&self) -> Result<bool, InstallMcpError> {
        json_has_server(&self.mcp_config_path(), "mcp").await
    }

    async fn uninstall_mcp(&self) -> Result<bool, InstallMcpError> {
        remove_from_config(&self.mcp_config_path(), "mcp").await
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
        let atuin = executable().await?;
        match self.mcp_program() {
            Some(program) => run(program, &["mcp", "add", "atuin", "--", atuin, "mcp"]).await,
            None => {
                let server = json!({"command": atuin, "args": ["mcp"]});
                add_to_config(&self.mcp_config_path(), json!({}), "mcpServers", server).await
            }
        }
    }

    async fn has_mcp(&self) -> Result<bool, InstallMcpError> {
        json_has_server(&self.mcp_config_path(), "mcpServers").await
    }

    /// From the file, which `pi mcp add` writes too.
    async fn uninstall_mcp(&self) -> Result<bool, InstallMcpError> {
        remove_from_config(&self.mcp_config_path(), "mcpServers").await
    }
}

/// The config file at `path`, as it is and its path with its links resolved, so that a write goes
/// where it is. `None` when there's no file.
async fn read_config(path: &Path) -> Result<Option<(PathBuf, Vec<u8>)>, InstallMcpError> {
    // A config linked from a dotfiles repo is saved there, keeping the link.
    let path =
        AbsPathBuf::canonicalize_async(path).await.map_or_else(|_| path.to_owned(), PathBuf::from);
    match tokio::fs::read(&path).await {
        Ok(read) => Ok(Some((path, read))),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err.into()),
    }
}

/// Write `text` over the config at `path` that read `read`, unless it's changed since.
fn rewrite_config(path: &Path, read: &[u8], text: &str) -> Result<(), InstallMcpError> {
    match crate::fs::replace(path, text.as_bytes(), |now| now == read) {
        Err(err) if !err.replaced() => Err(err.into()),
        _ => Ok(()),
    }
}

/// Whether the JSON config at `path` has an `atuin` server in its `key` object.
async fn json_has_server(path: &Path, key: &str) -> Result<bool, InstallMcpError> {
    let Some((path, read)) = read_config(path).await? else {
        return Ok(false);
    };
    let root: Value =
        serde_json::from_slice(&read).map_err(|err| InstallMcpError::Json(path, err))?;
    Ok(root.get(key).and_then(|servers| servers.get("atuin")).is_some())
}

/// Take the `atuin` server out of the `key` object of the JSON config at `path`, as
/// [`add_to_config`] puts it there. `false` when there was none.
async fn remove_from_config(path: &Path, key: &str) -> Result<bool, InstallMcpError> {
    let Some((path, read)) = read_config(path).await? else {
        return Ok(false);
    };
    let mut root: Value =
        serde_json::from_slice(&read).map_err(|err| InstallMcpError::Json(path.clone(), err))?;
    let servers = root.get_mut(key).and_then(Value::as_object_mut);
    if servers.and_then(|servers| servers.remove("atuin")).is_none() {
        return Ok(false);
    }
    let text = serde_json::to_string_pretty(&root)
        .map_err(|err| InstallMcpError::Json(path.clone(), err))?
        + "\n";
    rewrite_config(&path, &read, &text)?;
    crate::userio::log::info!("mcp-server-removed", path = path.display().to_string());
    Ok(true)
}

/// The TOML config at `path` as `mcp add` writes Codex's and Grok's, `None` when there's none.
async fn read_toml(
    path: &Path,
) -> Result<Option<(PathBuf, Vec<u8>, DocumentMut)>, InstallMcpError> {
    let Some((path, read)) = read_config(path).await? else {
        return Ok(None);
    };
    let doc = String::from_utf8_lossy(&read).parse::<DocumentMut>();
    let doc = doc.map_err(|err| InstallMcpError::Toml(path.clone(), err))?;
    Ok(Some((path, read, doc)))
}

/// Whether the TOML config at `path` has an `atuin` server in `mcp_servers`.
async fn toml_has_server(path: &Path) -> Result<bool, InstallMcpError> {
    let Some((_, _, doc)) = read_toml(path).await? else {
        return Ok(false);
    };
    Ok(doc.get("mcp_servers").and_then(|servers| servers.get("atuin")).is_some())
}

/// Take the `atuin` server out of `mcp_servers` in the TOML config at `path`, keeping the rest
/// of the file as it is. `false` when there was none.
async fn remove_toml_server(path: &Path) -> Result<bool, InstallMcpError> {
    let Some((path, read, mut doc)) = read_toml(path).await? else {
        return Ok(false);
    };
    let servers = doc.get_mut("mcp_servers").and_then(Item::as_table_like_mut);
    if servers.and_then(|servers| servers.remove("atuin")).is_none() {
        return Ok(false);
    }
    rewrite_config(&path, &read, &doc.to_string())?;
    crate::userio::log::info!("mcp-server-removed", path = path.display().to_string());
    Ok(true)
}

/// Whether the YAML config at `path` has an `atuin` server in `mcp_servers`, as Hermes's does.
async fn yaml_has_server(path: &Path) -> Result<bool, InstallMcpError> {
    let Some((path, read)) = read_config(path).await? else {
        return Ok(false);
    };
    let docs = yaml_rust2::YamlLoader::load_from_str(&String::from_utf8_lossy(&read))
        .map_err(|err| InstallMcpError::Yaml(path, err))?;
    Ok(docs.first().is_some_and(|doc| !doc["mcp_servers"]["atuin"].is_badvalue()))
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
/// an absolute path: see [`atuin_exe::launch_path`].
async fn executable() -> Result<&'static str, InstallMcpError> {
    let path = atuin_exe::launch_path().await?;
    path.to_str().ok_or_else(|| InstallMcpError::NonUtf8Executable(path.to_owned()))
}

/// Runs `program` with `args`, keeping what it prints off the terminal: a screen may be on it. The
/// program runs from the path that [`resolve_in_path`] finds, so that what runs is what the `PATH`
/// check found. For example, on Windows, `claude` runs as `claude.cmd`.
async fn run(program: &'static str, args: &[&str]) -> Result<(), InstallMcpError> {
    let found = resolve_in_path(Path::new(program)).ok_or(InstallMcpError::NotFound(program))?;
    let output =
        tokio::process::Command::new(&*found).args(args).stdin(Stdio::null()).output().await?;
    if output.status.success() {
        return Ok(());
    }
    let command = format!("{program} {}", args.join(" "));
    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(match stderr.lines().map(str::trim).find(|line| !line.is_empty()) {
        Some(said) => InstallMcpError::FailedSaying {
            command,
            said: said.to_owned(),
        },
        None => InstallMcpError::Failed(command),
    })
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

    /// Hermes's default home is `%LOCALAPPDATA%\\hermes` on Windows (here unset, so under the home's
    /// `AppData\\Local`), `~/.hermes` elsewhere.
    const HERMES_CONFIG: &str = if cfg!(windows) {
        "AppData/Local/hermes/config.yaml"
    } else {
        ".hermes/config.yaml"
    };

    /// A server atuin writes itself is found once registered, and not once unregistered.
    #[rstest]
    #[case("kiro")]
    #[case("opencode")]
    #[case("omp")]
    #[tokio::test]
    async fn a_server_atuin_writes_comes_out_as_it_went_in(
        agent_home: (tempfile::TempDir, MockEnv),
        #[case] name: &str,
    ) {
        let (_dir, _env) = agent_home;
        let harness = AnyHarness::from_name(name).unwrap();

        assert!(!harness.has_mcp().await.unwrap());
        harness.install_mcp().await.unwrap();
        assert!(harness.has_mcp().await.unwrap());
        assert!(harness.uninstall_mcp().await.unwrap());
        assert!(!harness.has_mcp().await.unwrap());
        assert!(!harness.uninstall_mcp().await.unwrap());
    }

    /// Unregistered from its file, atuin's server leaves the others there.
    #[rstest]
    #[case("copilot", ".copilot/mcp-config.json", "mcpServers")]
    #[case("pi", ".pi/agent/mcp.json", "mcpServers")]
    #[case("kiro", ".kiro/settings/mcp.json", "mcpServers")]
    #[case("opencode", ".config/opencode/opencode.json", "mcp")]
    #[tokio::test]
    async fn unregistering_keeps_the_other_servers(
        agent_home: (tempfile::TempDir, MockEnv),
        #[case] name: &str,
        #[case] relative: &str,
        #[case] key: &str,
    ) {
        let (dir, _env) = agent_home;
        let path = dir.path().join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let other = json!({"command": "other"});
        let config = json!({key: {"atuin": {"command": "atuin", "args": ["mcp"]}, "other": other}});
        std::fs::write(&path, config.to_string()).unwrap();
        let harness = AnyHarness::from_name(name).unwrap();

        assert!(harness.has_mcp().await.unwrap());
        assert!(harness.uninstall_mcp().await.unwrap());
        let left: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(left, json!({key: {"other": other}}));
    }

    /// Codex's and Grok's TOML keeps its comments and other servers.
    #[rstest]
    #[case("codex", ".codex/config.toml")]
    #[case("grok", ".grok/config.toml")]
    #[tokio::test]
    async fn unregistering_keeps_the_rest_of_a_toml_config(
        agent_home: (tempfile::TempDir, MockEnv),
        #[case] name: &str,
        #[case] relative: &str,
    ) {
        let (dir, _env) = agent_home;
        let path = dir.path().join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let config = "# mine\nmodel = \"o3\"\n\n[mcp_servers.atuin]\ncommand = \"atuin\"\nargs = \
                      [\"mcp\"]\n\n[mcp_servers.other]\ncommand = \"other\"\n";
        std::fs::write(&path, config).unwrap();
        let harness = AnyHarness::from_name(name).unwrap();

        assert!(harness.has_mcp().await.unwrap());
        assert!(harness.uninstall_mcp().await.unwrap());
        let left = std::fs::read_to_string(&path).unwrap();
        assert_eq!(left, "# mine\nmodel = \"o3\"\n\n[mcp_servers.other]\ncommand = \"other\"\n");
        assert!(!harness.has_mcp().await.unwrap());
    }

    /// Without `hermes` to take its server out, Hermes's is left to the user, as atuin can't write
    /// its config back with its comments.
    #[rstest]
    #[tokio::test]
    async fn hermes_without_its_cli_is_unregistered_by_hand(
        agent_home: (tempfile::TempDir, MockEnv),
    ) {
        let (dir, _env) = agent_home;
        let path = dir.path().join(HERMES_CONFIG);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "# mine\nmcp_servers:\n  atuin:\n    command: atuin\n").unwrap();
        let hermes = AnyHarness::from_name("hermes").unwrap();

        assert!(hermes.has_mcp().await.unwrap());
        let err = hermes.uninstall_mcp().await.unwrap_err();
        let by_hand = matches!(err, InstallMcpError::RemoveByHand {
            key: "mcp_servers.atuin",
            cause: HermesCliError::NotFound,
            ..
        });
        assert!(by_hand, "{err}");
    }

    #[rstest]
    #[case("amp", ".config/amp/settings.json")]
    #[case("claude-code", ".claude.json")]
    #[case("codex", ".codex/config.toml")]
    #[case("copilot", ".copilot/mcp-config.json")]
    #[case("grok", ".grok/config.toml")]
    #[case("hermes", HERMES_CONFIG)]
    #[case("kiro", ".kiro/settings/mcp.json")]
    #[case("opencode", ".config/opencode/opencode.json")]
    #[case("pi", ".pi/agent/mcp.json")]
    #[case("omp", ".omp/agent/mcp.json")]
    #[case("vibe", ".vibe/config.toml")]
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
    #[case("amp", "XDG_CONFIG_HOME", "amp/settings.json")]
    #[case("claude-code", "CLAUDE_CONFIG_DIR", ".claude.json")]
    #[case("codex", "CODEX_HOME", "config.toml")]
    #[case("copilot", "COPILOT_HOME", "mcp-config.json")]
    #[case("grok", "GROK_HOME", "config.toml")]
    #[case("hermes", "HERMES_HOME", "config.yaml")]
    #[case("kiro", "KIRO_HOME", "settings/mcp.json")]
    #[case("opencode", "XDG_CONFIG_HOME", "opencode/opencode.json")]
    #[case("pi", "PI_CODING_AGENT_DIR", "mcp.json")]
    #[case("omp", "PI_CODING_AGENT_DIR", "mcp.json")]
    #[case("vibe", "VIBE_HOME", "config.toml")]
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
