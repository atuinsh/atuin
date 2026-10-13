//! Trusting the hooks atuin installs for Codex. Since 0.160 Codex runs a hook from `hooks.json`
//! only once the user has trusted it: in its `config.toml`, under `[hooks.state."<key>"]`, a
//! `trusted_hash` of the hook as it was trusted, so a hook changed since runs no more. The user
//! asked for atuin's hooks by installing them, so atuin records them as trusted, each as it
//! installed it.
//!
//! The key is where the hook is: `<Codex home>/hooks.json:<event>:<group>:<handler>`, the home's
//! path with its symlinks resolved but not `hooks.json`'s own (as codex 0.160.1 keys them).

use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};
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

/// The Atuin command hooks, including their preserved Codex settings: each one's event,
/// group and handler position, and the hash Codex trusts it by.
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
                if handler["type"].as_str() != Some("command") {
                    continue;
                }
                let Ok(handler) = serde_json::from_value::<CommandHook>(handler.clone()) else {
                    continue;
                };
                if invokes_atuin_hook(handler.command(), harness) {
                    found.push((*key, g, h, hash(key, &group["matcher"], &handler)));
                }
            }
        }
    }
    found
}

/// Command settings read by Codex 0.160.1. Unknown fields do not enter its trust identity.
#[derive(serde::Deserialize)]
struct CommandHook {
    command: String,
    #[serde(default, rename = "commandWindows", alias = "command_windows")]
    command_windows: Option<String>,
    #[serde(default)]
    timeout: Option<u64>,
    #[serde(default)]
    r#async: bool,
    #[serde(default, rename = "statusMessage")]
    status_message: Option<String>,
    #[serde(default, rename = "additionalContextLimit")]
    additional_context_limit: Option<usize>,
}

impl CommandHook {
    fn command(&self) -> &str {
        if cfg!(windows) {
            self.command_windows.as_deref().unwrap_or(&self.command)
        } else {
            &self.command
        }
    }
}

/// Codex 0.160.1 hashes the normalized event, matcher and command handler with sorted keys.
/// These are tool-use events: timeout defaults to 600 seconds with a one-second floor,
/// and the default additional-context limit (2,500 tokens) is omitted from the identity.
fn hash(event: &str, matcher: &Value, handler: &CommandHook) -> String {
    let mut normalized = Map::from_iter([
        ("async".to_owned(), json!(handler.r#async)),
        ("command".to_owned(), json!(handler.command())),
        ("timeout".to_owned(), json!(handler.timeout.unwrap_or(600).max(1))),
        ("type".to_owned(), json!("command")),
    ]);
    if let Some(status) = &handler.status_message {
        normalized.insert("statusMessage".to_owned(), json!(status));
    }
    if let Some(limit) = handler.additional_context_limit.filter(|limit| *limit != 2_500) {
        normalized.insert("additionalContextLimit".to_owned(), json!(limit));
    }
    // serde_json may have preserve_order enabled elsewhere in the workspace.
    normalized.sort_keys();
    let mut identity = Map::from_iter([
        ("event_name".to_owned(), json!(event)),
        ("hooks".to_owned(), json!([normalized])),
    ]);
    if let Some(matcher) = matcher.as_str() {
        identity.insert("matcher".to_owned(), json!(matcher));
    }
    identity.sort_keys();
    let hook = Value::Object(identity);
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
        assert_eq!(hash(event, &json!("^Bash$"), &command_hook(command)), expected);
    }

    fn command_hook(command: &str) -> CommandHook {
        serde_json::from_value(json!({"command": command})).unwrap()
    }

    fn installed() -> Value {
        json!({"matcher": "^Bash$", "hooks": [{"type": "command", "command": COMMAND}]})
    }

    /// Golden identities follow Codex 0.160.1's command normalization, not the raw JSON.
    #[rstest]
    #[case::timeout(json!({"timeout":30}), "sha256:29dc1b3a621a1c5aedfd6a8f61961e44b0cb6243e7a957a54b5347578f2ef2f2")]
    #[case::timeout_floor(json!({"timeout":0}), "sha256:e72943f4b7d067333f3a65c175ac3ae683a759876ee07db21ff03a2e3b78c2a1")]
    #[case::asynchronous(json!({"async":true}), "sha256:c6a2e5227ffe10af0adabc0e349511707f6afbe168e58a61e29596440ce91489")]
    #[case::empty_status(json!({"statusMessage":""}), "sha256:68e0dc5f9a5d8e8c596e2e0394b894287e9791d2ccd2aadde3ed36f750cd1d81")]
    #[case::default_context_limit(json!({"additionalContextLimit":2500}), "sha256:3b981b43c4878ddcca45ed5c03b71b76508a73e7c8492cdb3266c0867adade82")]
    #[case::custom_context_limit(json!({"additionalContextLimit":4096}), "sha256:ada83ccad31af778dd7e3a69a1d9c4df2e758b57c241c5a299268148901d76f0")]
    #[case::combined(json!({"timeout":30,"async":true,"statusMessage":"Recording command","additionalContextLimit":0}), "sha256:9c4b6250f2eb317714ddffa742e4005df2c2bebf2e5aac6402678e1b2744986c")]
    fn custom_hashes_follow_codex_normalization(#[case] extras: Value, #[case] expected: &str) {
        let mut handler = json!({"type": "command", "command": COMMAND});
        handler.as_object_mut().unwrap().extend(extras.as_object().unwrap().clone());
        let handler = serde_json::from_value(handler).unwrap();
        assert_eq!(hash("pre_tool_use", &json!("^Bash$"), &handler), expected);
    }

    #[rstest]
    fn missing_matchers_and_ignored_fields_follow_codexs_identity() {
        let hooks = json!({"hooks": {"PreToolUse": [{"hooks": [{
            "type": "command", "command": COMMAND, "timeout": 30, "shell": "bash"
        }]}]}});
        let trusted = atuin_hooks(&hooks, "codex");
        assert_eq!(trusted.len(), 1);
        assert_eq!(
            trusted[0].3,
            "sha256:63eea776b805f3df375ced6364309975e816180dd463d07bb7778d89e3d5e377"
        );
    }

    #[rstest]
    #[case::other_command(json!({"type": "command", "command": "echo other", "timeout": 30}))]
    #[case::other_harness(json!({"type": "command", "command": "atuin hook claude-code"}))]
    #[case::other_type(json!({"type": "prompt", "command": COMMAND}))]
    #[case::invalid_timeout(json!({"type": "command", "command": COMMAND, "timeout": -1}))]
    fn unrelated_or_invalid_handlers_are_not_trusted(#[case] handler: Value) {
        let hooks = json!({"hooks": {"PreToolUse": [{"hooks": [handler]}]}});
        assert!(atuin_hooks(&hooks, "codex").is_empty());
    }

    #[rstest]
    #[case::camel_case("commandWindows")]
    #[case::snake_case("command_windows")]
    fn windows_overrides_are_checked_before_trusting(#[case] field: &str) {
        let mut handler = json!({"type": "command", "command": COMMAND, "timeout": 30});
        handler[field] = json!("echo other");
        let hooks = json!({"hooks": {"PreToolUse": [{"matcher": "^Bash$", "hooks": [handler]}]}});
        let trusted = atuin_hooks(&hooks, "codex");
        if cfg!(windows) {
            assert!(trusted.is_empty());
        } else {
            assert_eq!(trusted.len(), 1);
            assert_eq!(
                trusted[0].3,
                "sha256:29dc1b3a621a1c5aedfd6a8f61961e44b0cb6243e7a957a54b5347578f2ef2f2"
            );
        }
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
                hash("pre_tool_use", &json!("^Bash$"), &command_hook(COMMAND))
            ),
            (
                format!("{}:post_tool_use:0:0", path.display()),
                hash("post_tool_use", &json!("^Bash$"), &command_hook(COMMAND))
            ),
        ]);
        assert!(text.starts_with(before.lines().next().unwrap()), "{text}");
    }

    /// Reinstalling a customized command must renew its trust without enabling disabled hooks.
    #[rstest]
    #[tokio::test]
    async fn customized_hooks_are_retrusted_after_command_changes() {
        let home = tempfile::tempdir().unwrap();
        let hooks = json!({"hooks": {"PreToolUse": [{
            "matcher": "Bash|Shell",
            "hooks": [{"type": "command", "command": COMMAND, "timeout": 30,
                "async": true, "statusMessage": "Recording command", "additionalContextLimit": 0}]
        }]}});
        let hooks_path = home.path().join("hooks.json");
        std::fs::write(&hooks_path, hooks.to_string()).unwrap();
        let key = format!(
            "{}:pre_tool_use:0:0",
            std::fs::canonicalize(home.path()).unwrap().join("hooks.json").display()
        );
        let mut before = DocumentMut::new();
        before["hooks"]["state"][&key]["enabled"] = toml_edit::value(false);
        let old_hash = "sha256:be6eee6c4df7a484ae37b85e9c4be021578df42255fc308fcc3bab527eb86102";
        before["hooks"]["state"][&key]["trusted_hash"] = toml_edit::value(old_hash);
        std::fs::write(home.path().join("config.toml"), before.to_string()).unwrap();

        let config = config(home.path()).await.unwrap();
        let mut changed_hooks = hooks.clone();
        changed_hooks["hooks"]["PreToolUse"][0]["hooks"][0]["command"] =
            json!("/opt/atuin-upgrade/bin/atuin hook codex");
        std::fs::write(&hooks_path, changed_hooks.to_string()).unwrap();
        assert!(trust(home.path(), "codex", config).await.unwrap());
        let installed: Value =
            serde_json::from_str(&std::fs::read_to_string(&hooks_path).unwrap()).unwrap();
        let handler = &installed["hooks"]["PreToolUse"][0]["hooks"][0];
        let expected = "sha256:f82a42488fc67cc8c1cc04a7fda677bd1fc9e8a23d6fe999eca24594e2c439ed";
        let text = std::fs::read_to_string(home.path().join("config.toml")).unwrap();
        let after: DocumentMut = text.parse().unwrap();
        assert_eq!(after["hooks"]["state"][&key]["trusted_hash"].as_str(), Some(expected));
        assert_eq!(after["hooks"]["state"][&key]["enabled"].as_bool(), Some(false));
        assert_eq!(handler["timeout"], 30);
        assert_eq!(handler["async"], true);
        assert_eq!(handler["statusMessage"], "Recording command");
        assert_eq!(handler["additionalContextLimit"], 0);
        assert!(
            !trust(home.path(), "codex", super::config(home.path()).await.unwrap()).await.unwrap()
        );
        assert_eq!(std::fs::read_to_string(home.path().join("config.toml")).unwrap(), text);
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
