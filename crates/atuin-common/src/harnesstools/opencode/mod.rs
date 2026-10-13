use std::path::{Path, PathBuf};

use super::rehydrate::{RehydrateError, RehydrateSession};
use super::resume::{self, CwdRequirement, ResumeError, ResumePlan, ResumeTarget};
use super::{Harness, InstallHookError};
use crate::dirs::home_dir;
use crate::env::var_nonempty;

pub mod rehydrate;
pub mod session;

#[derive(Debug, Clone, Copy, Default)]
pub struct Opencode;

impl Opencode {
    #[must_use]
    pub fn new() -> Self {
        Self {}
    }
}

/// The plugin for opencode 1.x, whose loaders (up to 1.3) call every export of a plugin file.
const PLUGIN_1: &str = include_str!("../../../res/harnesstools/opencode/atuin.ts");
/// The plugin for opencode 2, which loads a default export's `id` and `setup`.
const PLUGIN_2: &str = include_str!("../../../res/harnesstools/opencode/atuin.v2.ts");

/// The plugin for the opencode of major version `major`: no export shape loads in both. One
/// whose version can't be told (not installed yet) gets 1.x's, which npm still ships.
const fn plugin_for(major: Option<u64>) -> &'static str {
    match major {
        Some(major) if major >= 2 => PLUGIN_2,
        _ => PLUGIN_1,
    }
}

impl Harness for Opencode {
    fn name(&self) -> &'static str {
        "opencode"
    }

    /// Install the hooks on the current active user's machine
    async fn install_hooks(&self) -> Result<PathBuf, InstallHookError> {
        const PLUGIN_NAME: &str = "atuin.ts";
        let plugin = plugin_for(rehydrate::major_version(Path::new("opencode")).await.ok());

        // Opencode is XDG-compliant: plugins live under $XDG_CONFIG_HOME/opencode,
        // falling back to ~/.config/opencode.
        let plugins_dir = var_nonempty("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home_dir().join(".config"))
            .join("opencode")
            .join("plugins");

        tokio::fs::create_dir_all(&plugins_dir).await?;

        let extension_path = plugins_dir.join(PLUGIN_NAME);

        let already_installed = tokio::fs::read_to_string(&extension_path)
            .await
            .is_ok_and(|existing| existing == plugin);

        if already_installed {
            return Err(InstallHookError::AlreadyInstalled);
        }

        tokio::fs::write(&extension_path, plugin).await?;

        Ok(extension_path)
    }

    /// `opencode --session <id>`. opencode looks the id up in its one global database and serves
    /// the session from the directory recorded for it, but the TUI starts in the directory it is
    /// run from (its project's session list, config and the new sessions it creates), so the
    /// session's own is the one to be in.
    fn resume_plan(&self, target: &ResumeTarget) -> Result<ResumePlan, ResumeError> {
        let args = ["--session".to_owned(), target.id.clone()];
        Ok(resume::plan(target, CwdRequirement::Preferred, "opencode", args))
    }

    async fn locate(&self, id: &str) -> Option<PathBuf> {
        session::locate(&session::default_db()?, id).await
    }

    async fn rehydrate(&self, session: &RehydrateSession) -> Result<PathBuf, RehydrateError> {
        rehydrate::rehydrate(session).await
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    /// Each opencode gets the plugin its loader takes: 1.x (and one whose version can't be told)
    /// a named plugin function, the only export; 2 a default export with an `id` and `setup`.
    #[rstest]
    #[case::none(None, PLUGIN_1)]
    #[case::one(Some(1), PLUGIN_1)]
    #[case::two(Some(2), PLUGIN_2)]
    #[case::three(Some(3), PLUGIN_2)]
    fn each_opencode_gets_the_plugin_it_loads(#[case] major: Option<u64>, #[case] want: &str) {
        assert_eq!(plugin_for(major), want);
        let exports = |plugin: &str| -> Vec<String> {
            plugin.lines().filter(|l| l.starts_with("export ")).map(str::to_owned).collect()
        };
        assert_eq!(exports(PLUGIN_1), [
            "export const AtuinPlugin: Plugin = async ({ directory }) => {"
        ]);
        assert_eq!(exports(PLUGIN_2), ["export default { id: \"atuin\", setup };"]);
    }

    /// Whether `node` is here and runs TypeScript as it is.
    fn node_runs_typescript() -> bool {
        std::process::Command::new("node")
            .args(["-e", "process.exit(process.features.typescript ? 0 : 1)"])
            .status()
            .is_ok_and(|status| status.success())
    }

    /// A fake opencode 2 driving the plugin through its hooks as opencode calls them for a shell
    /// call (`execute.before`, `create.before`, `execute.after`): one call per way a call ends.
    const DRIVER: &str = r#"
import plugin from "./atuin.ts";
const hooks = {};
const add = (kind) => (name, fn) => { (hooks[`${kind}.${name}`] ??= []).push(fn); };
plugin.setup({ tool: { hook: add("tool") }, shell: { hook: add("shell") } });
const fire = async (name, payload) => { for (const fn of hooks[name] ?? []) await fn(payload); };
async function call(id, command, after) {
    await fire("tool.execute.before", { tool: "shell", id, input: { command, description: `about ${id}` } });
    const shell = { command, cwd: "." };
    await fire("shell.create.before", shell);
    shell.cwd = process.cwd();
    await fire("tool.execute.after", { tool: "shell", id, input: { command }, ...after });
}
const done = (metadata) => ({ status: "completed", result: { content: [], metadata } });
await call("c1", "true", done({ status: "completed", truncated: false, exit: 0 }));
await call("c2", "false", done({ status: "completed", truncated: false, exit: 1 }));
await call("c3", "sleep 999", done({ status: "completed", truncated: false, timeout: true, signal: "SIGTERM" }));
await call("c4", "npm run dev", done({ status: "running", truncated: false, shellID: "sh_1" }));
await call("c5", "rm -rf x", { status: "error", error: new Error("denied") });
await fire("tool.execute.after", { tool: "read", id: "c6", status: "completed", result: {} });
"#;

    /// The opencode 2 plugin records each shell command that ended, with its exit code (a
    /// timeout's 124); not one left running in the background, nor one that was denied.
    #[cfg(unix)]
    #[rstest]
    fn the_opencode_2_plugin_records_commands_that_ended() {
        use std::os::unix::fs::PermissionsExt;
        if !node_runs_typescript() {
            eprintln!("skipped: no node that runs TypeScript");
            return;
        }
        // next to the test binary: CI's temporary directory is mounted without exec
        let exe = std::env::current_exe().unwrap();
        let dir = tempfile::tempdir_in(exe.parent().unwrap()).unwrap();
        let log = dir.path().join("atuin.log");
        let stub = dir.path().join("atuin");
        std::fs::write(
            &stub,
            format!(
                "#!/bin/sh\necho \"$@\" >> {log}\n[ \"$2\" = start ] && echo h_1\nexit 0\n",
                log = log.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(dir.path().join("atuin.ts"), PLUGIN_2).unwrap();
        std::fs::write(dir.path().join("driver.mts"), DRIVER).unwrap();
        let path = crate::env::var("PATH").unwrap_or_default();

        let ran = std::process::Command::new("node")
            .arg("driver.mts")
            .current_dir(dir.path())
            .env("PATH", format!("{}:{path}", dir.path().display()))
            .output()
            .unwrap();
        assert!(ran.status.success(), "{}", String::from_utf8_lossy(&ran.stderr));

        let calls: Vec<String> = std::fs::read_to_string(&log)
            .unwrap()
            .lines()
            .map(|line| match line.split_once(" --duration ") {
                // how long it took is not the test's business
                Some((before, nanos)) => {
                    assert!(nanos.parse::<u64>().is_ok(), "{line}");
                    format!("{before} --duration _")
                }
                None => line.to_owned(),
            })
            .collect();
        let start = |id: &str, command: &str| {
            format!(
                "history start --author opencode --author-kind agent --intent about {id} -- \
                 {command}"
            )
        };
        let end = |exit: i32| format!("history end h_1 --exit {exit} --duration _");
        assert_eq!(calls, [
            start("c1", "true"),
            end(0),
            start("c2", "false"),
            end(1),
            start("c3", "sleep 999"),
            end(124),
        ]);
    }
}
