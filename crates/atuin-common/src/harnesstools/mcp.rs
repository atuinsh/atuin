//! Register atuin's MCP server (`atuin mcp`) with a harness, so its agent can search the user's
//! history, command output and agent sessions.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use serde_json::{Value, json};

use super::pi::Flavour;
use super::{AnyHarness, opencode};
use crate::path::PathBufExt as _;

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

/// Register `atuin mcp` with `harness` at user scope, replacing any `atuin` server already there,
/// so a rerun picks up a moved binary.
pub async fn install(harness: AnyHarness) -> Result<(), InstallMcpError> {
    let atuin = executable()?;
    let atuin = atuin.as_str();
    match harness {
        AnyHarness::ClaudeCode(_) => {
            // `claude mcp add` refuses a name that's taken.
            let remove = ["mcp", "remove", "--scope", "user", "atuin"];
            let _ = tokio::process::Command::new("claude")
                .args(remove)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .await;
            run("claude", &["mcp", "add", "--scope", "user", "atuin", "--", atuin, "mcp"]).await
        }
        AnyHarness::Codex(_) => run("codex", &["mcp", "add", "atuin", "--", atuin, "mcp"]).await,
        // Not `opencode mcp add`: 1.x's only prompts, and 2.0's writes `mcp.servers`, which 2.0
        // doesn't load. Both load `mcp.<name>`.
        AnyHarness::Opencode(_) => {
            install_opencode(&opencode::config_dir().joined("opencode.json"), atuin).await
        }
        AnyHarness::Pi(pi) => match pi.flavour() {
            Flavour::Pi => run("pi", &["mcp", "add", "atuin", "--", atuin, "mcp"]).await,
            // omp has no `mcp add`: its own `/mcp add` writes this file.
            Flavour::Omp => {
                let path = Flavour::Omp.agent_dir().joined("mcp.json");
                let server = json!({"command": atuin, "args": ["mcp"]});
                add_to_config(&path, json!({}), "mcpServers", server).await
            }
        },
    }
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
        Some(on_path) if on_path.canonicalize().ok() == exe.canonicalize().ok() => on_path,
        _ => exe,
    }
}

/// Run `program` with `args`, letting it report what it did.
async fn run(program: &'static str, args: &[&str]) -> Result<(), InstallMcpError> {
    let status = tokio::process::Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .status()
        .await
        .map_err(|err| match err.kind() {
            std::io::ErrorKind::NotFound => InstallMcpError::NotFound(program),
            _ => err.into(),
        })?;
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

/// Add `server` as `atuin` to the `key` object of the JSON config at `path`, `new` when there is
/// none yet.
async fn add_to_config(
    path: &Path,
    new: Value,
    key: &'static str,
    server: Value,
) -> Result<(), InstallMcpError> {
    // A config linked from a dotfiles repo is saved there, keeping the link.
    let path = &tokio::fs::canonicalize(path).await.unwrap_or_else(|_| path.to_owned());
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
    println!("Added MCP server atuin to {}", path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;
    use crate::env::mock::MockEnv;

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
