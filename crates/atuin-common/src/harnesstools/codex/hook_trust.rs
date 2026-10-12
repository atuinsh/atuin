//! Trusting the hooks atuin installs for Codex. Since 0.160 Codex runs a hook from `hooks.json`
//! only once the user has trusted it: in its `config.toml`, under `[hooks.state."<key>"]`, a
//! `trusted_hash` of the hook as it was trusted, so a hook changed since runs no more. The user
//! asked for atuin's hooks by installing them, so atuin records them as trusted, each as it
//! installed it.
//!
//! The key is where the hook is: `<Codex home>/hooks.json:<event>:<group>:<handler>`, the home's
//! path with its symlinks resolved but not `hooks.json`'s own (as codex 0.160.1 keys them).

use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use toml_edit::{DocumentMut, Item, Table, TableLike};

use crate::harnesstools::InstallHookError;
use crate::harnesstools::json_hooks::invokes_atuin_hook;

/// The events Codex runs atuin's hooks for, as `hooks.json` and the trust key name them.
const EVENTS: &[(&str, &str)] = &[("PreToolUse", "pre_tool_use"), ("PostToolUse", "post_tool_use")];

/// Codex's `config.toml`, read before anything is installed, so a config atuin can't record
/// trust in leaves nothing half done.
pub(super) struct Config {
    path: PathBuf,
    doc: DocumentMut,
}

const MALFORMED: InstallHookError =
    InstallHookError::Malformed("`hooks.state` in Codex's config.toml is not a table");

/// The `config.toml` of Codex home `home` (empty where there is none yet), refused where it
/// isn't TOML or holds a `hooks.state` that isn't a table.
pub(super) async fn config(home: &Path) -> Result<Config, InstallHookError> {
    let path = home.join("config.toml");
    let text = match tokio::fs::read_to_string(&path).await {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(err) => return Err(err.into()),
    };
    let mut doc: DocumentMut =
        text.parse().map_err(|_| InstallHookError::Malformed("Codex's config.toml is not TOML"))?;
    state(&mut doc)?;
    Ok(Config { path, doc })
}

/// The `hooks.state` table of `doc`, made where there is none (inline or not, as it is).
fn state(doc: &mut DocumentMut) -> Result<&mut dyn TableLike, InstallHookError> {
    doc.entry("hooks")
        .or_insert_with(implicit)
        .as_table_like_mut()
        .and_then(|hooks| hooks.entry("state").or_insert_with(implicit).as_table_like_mut())
        .ok_or(MALFORMED)
}

/// Record the hooks running `atuin hook <harness>` in `home/hooks.json` as trusted in `config`,
/// returning whether that changed it.
pub(super) async fn trust(
    home: &Path,
    harness: &str,
    mut config: Config,
) -> Result<bool, InstallHookError> {
    let hooks_path = tokio::fs::canonicalize(home).await?.join("hooks.json");
    let hooks: Value = serde_json::from_str(&tokio::fs::read_to_string(&hooks_path).await?)?;
    let mut changed = false;
    let state = state(&mut config.doc)?;
    for (event, group, handler, hash) in atuin_hooks(&hooks, harness) {
        let key = format!("{}:{event}:{group}:{handler}", hooks_path.display());
        let hook = state
            .entry(&key)
            .or_insert_with(|| Item::Table(Table::new()))
            .as_table_like_mut()
            .ok_or(InstallHookError::Malformed("a hook's state in Codex's config.toml"))?;
        if hook.get("trusted_hash").and_then(Item::as_str) != Some(hash.as_str()) {
            hook.insert("trusted_hash", toml_edit::value(hash));
            changed = true;
        }
    }
    if changed {
        tokio::fs::write(&config.path, config.doc.to_string()).await?;
    }
    Ok(changed)
}

fn implicit() -> Item {
    let mut table = Table::new();
    table.set_implicit(true);
    Item::Table(table)
}

/// The hooks running `atuin hook <harness>` in a `hooks.json`, as atuin installs them (a
/// command alone): each one's event, as the trust key names it, where it is (its group and its
/// place in it), and the hash Codex trusts it by.
fn atuin_hooks(hooks: &Value, harness: &str) -> Vec<(&'static str, usize, usize, String)> {
    let mut found = Vec::new();
    for (event, key) in EVENTS {
        let Some(groups) = hooks["hooks"][event].as_array() else {
            continue;
        };
        for (g, group) in groups.iter().enumerate() {
            let Some(handlers) = group["hooks"].as_array() else {
                continue;
            };
            for (h, handler) in handlers.iter().enumerate() {
                let Some(command) = handler["command"].as_str() else {
                    continue;
                };
                let installed = *handler == json!({"type": "command", "command": command});
                if installed && invokes_atuin_hook(command, harness) {
                    found.push((*key, g, h, hash(key, &group["matcher"], command)));
                }
            }
        }
    }
    found
}

/// What Codex trusts a hook by: the SHA-256 of the JSON of its event, matcher and handler (its
/// defaults filled in), keys sorted.
fn hash(event: &str, matcher: &Value, command: &str) -> String {
    let hook = json!({
        "event_name": event,
        "hooks": [{"async": false, "command": command, "timeout": 600, "type": "command"}],
        "matcher": matcher,
    });
    format!("sha256:{:x}", Sha256::digest(hook.to_string()))
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    const COMMAND: &str = "/opt/atuin/bin/atuin hook codex";

    /// The hashes codex 0.160.1 itself gave these hooks, as its app server's `hooks/list`
    /// reported them for a `hooks.json` holding them (`currentHash`): of the command a dev build
    /// installed, and of [`COMMAND`].
    #[rstest]
    #[case::pre_dev(
        "pre_tool_use",
        "/home/ellie/src/mono/.claude/worktrees/agent-a6cdedcfcffbcf53a/atuin/target/debug/atuin \
         hook codex",
        "sha256:5c43193105a163fcd3fed7f209bb1b1169cece43716091bc6dac24d2a7a059ab"
    )]
    #[case::post_dev(
        "post_tool_use",
        "/home/ellie/src/mono/.claude/worktrees/agent-a6cdedcfcffbcf53a/atuin/target/debug/atuin \
         hook codex",
        "sha256:3621fd71768b67a9c87d2732c34f77bd5403afb7f27226388dced73e52c18c78"
    )]
    #[case::pre(
        "pre_tool_use",
        COMMAND,
        "sha256:3b981b43c4878ddcca45ed5c03b71b76508a73e7c8492cdb3266c0867adade82"
    )]
    #[case::post(
        "post_tool_use",
        COMMAND,
        "sha256:274cc2128367ee69da364e7677ebe7489cc8f29de94635225f2e40735e09e642"
    )]
    fn hashes_are_codexs(#[case] event: &str, #[case] command: &str, #[case] expected: &str) {
        assert_eq!(hash(event, &json!("^Bash$"), command), expected);
    }

    fn installed() -> Value {
        json!({"matcher": "^Bash$", "hooks": [{"type": "command", "command": COMMAND}]})
    }

    /// Atuin's hooks are trusted where they are, beside the user's, which are left untrusted;
    /// done again, nothing changes. A `hooks` table written inline is written to as it is.
    #[rstest]
    #[case::tables("model = \"o3\"\n")]
    #[case::inline("model = \"o3\"\nhooks = { state = {} }\n")]
    #[tokio::test]
    async fn atuins_hooks_are_trusted_where_they_are(#[case] before: &str) {
        let home = tempfile::tempdir().unwrap();
        let theirs = json!({"matcher": "", "hooks": [{"type": "command", "command": "echo hi"}]});
        let hooks = json!({"hooks": {
            "PreToolUse": [theirs, installed()],
            "PostToolUse": [installed()],
            "PostToolUseFailure": [installed()],
        }});
        std::fs::write(home.path().join("hooks.json"), hooks.to_string()).unwrap();
        std::fs::write(home.path().join("config.toml"), before).unwrap();

        let config_of = || config(home.path());
        assert!(trust(home.path(), "codex", config_of().await.unwrap()).await.unwrap());
        assert!(!trust(home.path(), "codex", config_of().await.unwrap()).await.unwrap());

        let path = std::fs::canonicalize(home.path()).unwrap().join("hooks.json");
        let text = std::fs::read_to_string(home.path().join("config.toml")).unwrap();
        let config: toml_edit::DocumentMut = text.parse().unwrap();
        let state = config["hooks"]["state"].as_table_like().unwrap();
        let trusted: Vec<(String, String)> = state
            .iter()
            .map(|(key, hook)| (key.to_owned(), hook["trusted_hash"].as_str().unwrap().to_owned()))
            .collect();
        pretty_assertions::assert_eq!(trusted, [
            (
                format!("{}:pre_tool_use:1:0", path.display()),
                hash("pre_tool_use", &json!("^Bash$"), COMMAND)
            ),
            (
                format!("{}:post_tool_use:0:0", path.display()),
                hash("post_tool_use", &json!("^Bash$"), COMMAND)
            ),
        ]);
        assert!(text.starts_with(before.lines().next().unwrap()), "{text}");
    }

    /// Codex keys a hook by its home's real path, `hooks.json` itself as it is named there, even
    /// where that is a link (a dotfiles checkout's).
    #[cfg(unix)]
    #[rstest]
    #[tokio::test]
    async fn the_key_names_hooks_json_in_codexs_real_home() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real");
        std::fs::create_dir(&real).unwrap();
        std::os::unix::fs::symlink(&real, dir.path().join("home")).unwrap();
        let home = dir.path().join("home");
        let hooks = json!({"hooks": {"PreToolUse": [installed()]}});
        std::fs::write(dir.path().join("dotfiles.json"), hooks.to_string()).unwrap();
        std::os::unix::fs::symlink(dir.path().join("dotfiles.json"), real.join("hooks.json"))
            .unwrap();

        trust(&home, "codex", config(&home).await.unwrap()).await.unwrap();
        let text = std::fs::read_to_string(real.join("config.toml")).unwrap();
        let real = std::fs::canonicalize(&real).unwrap();
        assert!(text.contains(&format!("{}:pre_tool_use:0:0", real.join("hooks.json").display())));
    }

    /// A config whose `hooks.state` isn't a table is refused before anything is written.
    #[rstest]
    #[case::not_toml("model = ")]
    #[case::state_not_a_table("hooks = { state = 1 }\n")]
    #[tokio::test]
    async fn a_config_trust_cant_be_recorded_in_is_refused(#[case] text: &str) {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(home.path().join("config.toml"), text).unwrap();
        assert!(matches!(config(home.path()).await, Err(InstallHookError::Malformed(_))));
    }
}
