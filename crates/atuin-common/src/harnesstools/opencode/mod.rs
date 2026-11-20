use std::path::{Path, PathBuf};

use super::rehydrate::{RehydrateError, RehydrateSession, Rehydrated};
use super::resume::{self, CwdRequirement, ResumeError, ResumePlan, ResumeTarget};
use super::{Harness, InstallHookError, TS_ATUIN_LINE, plugin_with_executable};
use crate::atuin_exe::launch_path;
use crate::dirs::home_dir;
use crate::env::var_nonempty;
use crate::path::PathBufExt as _;

pub mod rehydrate;
pub mod session;

/// Which opencode-shaped CLI: opencode itself, or Kilo Code, a fork of opencode that keeps
/// opencode's session format in directories of its own. (Their data dirs are
/// `$XDG_DATA_HOME/opencode` / `$XDG_DATA_HOME/kilo`; see `Flavour::data_dir` methods.)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flavour {
    Opencode,
    Kilo,
}

impl Flavour {
    /// The command that runs it, which is also its name (and its config/data dir name).
    #[must_use]
    pub const fn binary(self) -> &'static str {
        match self {
            Self::Opencode => "opencode",
            Self::Kilo => "kilo",
        }
    }

    /// The harness's name as people know it (`Harness::display_name`).
    #[must_use]
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Opencode => "opencode",
            Self::Kilo => "Kilo Code",
        }
    }

    /// The configuration directory: opencode's is XDG-compliant
    /// (`$XDG_CONFIG_HOME/opencode`, falling back to `~/.config/opencode`); Kilo's is
    /// `$KILO_CONFIG_DIR` if set (what Kilo itself resolves), else `$XDG_CONFIG_HOME/kilo`.
    pub(crate) fn config_dir(self) -> PathBuf {
        match self {
            Self::Opencode => var_nonempty("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| home_dir().joined(".config"))
                .joined("opencode"),
            Self::Kilo => var_nonempty("KILO_CONFIG_DIR").map_or_else(
                || {
                    var_nonempty("XDG_CONFIG_HOME")
                        .map(PathBuf::from)
                        .unwrap_or_else(|| home_dir().joined(".config"))
                        .joined("kilo")
                },
                PathBuf::from,
            ),
        }
    }

    /// The config file whose `mcp` key atuin writes (see `InstallMcp`).
    pub(crate) const fn mcp_config_file(self) -> &'static str {
        match self {
            Self::Opencode => "opencode.json",
            Self::Kilo => "kilo.json",
        }
    }

    /// The `$schema` the mcp file of that JSON gets.
    pub(crate) const fn mcp_schema(self) -> &'static str {
        match self {
            Self::Opencode => "https://opencode.ai/config.json",
            Self::Kilo => "https://app.kilo.ai/config.json",
        }
    }

    /// The environment variable that names its session database, as atuin finds it
    /// (`OPENCODE_DB` / `KILO_DB`; see `OpencodeSessions::resolve_db`).
    pub(crate) const fn db_env(self) -> &'static str {
        match self {
            Self::Opencode => "OPENCODE_DB",
            Self::Kilo => "KILO_DB",
        }
    }

    /// The environment variable that makes it write only `<stem>.db` and never a channel
    /// database (`OPENCODE_DISABLE_CHANNEL_DB` / `KILO_DISABLE_CHANNEL_DB`).
    pub(crate) const fn channel_db_env(self) -> &'static str {
        match self {
            Self::Opencode => "OPENCODE_DISABLE_CHANNEL_DB",
            Self::Kilo => "KILO_DISABLE_CHANNEL_DB",
        }
    }

    /// The stem of its database files (`opencode.db`, `opencode-<channel>.db`), or `kilo.db`,
    /// `kilo-<channel>.db`. Kilo resolves its database the way opencode 1.x does
    /// (see `getChannelPath` in `packages/opencode/src/storage/db.ts`).
    pub(crate) const fn db_stem(self) -> &'static str {
        match self {
            Self::Opencode => "opencode",
            Self::Kilo => "kilo",
        }
    }

    /// Whether this CLI has opencode 2's session layout: `session_v2`/`session_message` rows
    /// and `opencode session import`. Kilo never does: it writes only opencode 1.x's `session`,
    /// `message` and `part` tables (its `session_message` table is created but left empty), so a
    /// Kilo session is always a 1.x session, and atuin writes one out whole with `kilo import`
    /// (the way opencode 1.x does).
    pub(crate) const fn has_v2_session(self) -> bool {
        matches!(self, Self::Opencode)
    }

    /// The environment kept quiet for a spawned CLI: no update check, no model catalogue
    /// refresh, no LSP download, no external plugins — and no sharing of the session it is
    /// asked to import. Kilo takes opencode's own flags with a `KILO_` prefix, and additionally
    /// its local-server ingest is disabled (`KILO_DISABLE_SESSION_INGEST=1`), so importing a
    /// session does not POST it to Kilo's cloud.
    pub(crate) const fn quiet_env(self) -> &'static [(&'static str, &'static str)] {
        match self {
            Self::Opencode => OPENCODE_QUIET_ENV,
            Self::Kilo => KILO_QUIET_ENV,
        }
    }

    /// The environment variable that makes it share the session it imports, which atuin removes
    /// from the child process (so a restore does not share it either).
    pub(crate) const fn auto_share_env(self) -> &'static str {
        match self {
            Self::Opencode => "OPENCODE_AUTO_SHARE",
            Self::Kilo => "KILO_AUTO_SHARE",
        }
    }
}

/// opencode's quiet environment (see `Flavour::quiet_env`).
const OPENCODE_QUIET_ENV: &[(&str, &str)] = &[
    ("OPENCODE_DISABLE_AUTOUPDATE", "1"),
    ("OPENCODE_DISABLE_MODELS_FETCH", "1"),
    ("OPENCODE_DISABLE_LSP_DOWNLOAD", "1"),
    ("OPENCODE_DISABLE_DEFAULT_PLUGINS", "1"),
    ("OPENCODE_DISABLE_SHARE", "1"),
    ("OPENCODE_PURE", "1"),
];

/// Kilo's quiet environment (see `Flavour::quiet_env`). Kilo takes opencode's flags with a
/// `KILO_` prefix, and sharing/ingest are disabled too (`KILO_DISABLE_SHARE`,
/// `KILO_DISABLE_SESSION_INGEST`), so importing a session does not POST it to Kilo's cloud.
const KILO_QUIET_ENV: &[(&str, &str)] = &[
    ("KILO_DISABLE_AUTOUPDATE", "1"),
    ("KILO_DISABLE_MODELS_FETCH", "1"),
    ("KILO_DISABLE_LSP_DOWNLOAD", "1"),
    ("KILO_DISABLE_DEFAULT_PLUGINS", "1"),
    ("KILO_DISABLE_SHARE", "1"),
    ("KILO_DISABLE_SESSION_INGEST", "1"),
    ("KILO_PURE", "1"),
];

#[derive(Debug, Clone, Copy)]
pub struct Opencode {
    flavour: Flavour,
}

impl Opencode {
    pub const OPENCODE: Self = Self {
        flavour: Flavour::Opencode,
    };
    pub const KILO: Self = Self {
        flavour: Flavour::Kilo,
    };

    #[must_use]
    pub const fn flavour(self) -> Flavour {
        self.flavour
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

/// The plugin a `flavour` of major version `major` loads: Kilo is 1.x-shaped, so it only ever
/// loads 1.x's; opencode's choice depends on its own version (see `plugin_for`).
fn plugin_flavour(flavour: Flavour, major: Option<u64>) -> String {
    match flavour {
        Flavour::Opencode => plugin_for(major).to_owned(),
        // Kilo records its commands under the `kilo` author, so swap the shared template's
        // hardcoded `opencode` author for Kilo's.
        Flavour::Kilo => PLUGIN_1
            .replace("const ATUIN_AUTHOR = \"opencode\";", "const ATUIN_AUTHOR = \"kilo\";"),
    }
}

impl Harness for Opencode {
    fn name(&self) -> &'static str {
        self.flavour.binary()
    }

    fn display_name(&self) -> &'static str {
        self.flavour.display_name()
    }

    /// No, and it can't: opencode keeps no fork link of its own (`opencode import` writes none),
    /// and atuin's, the fork's marker, is a row whose text capture doesn't keep, so sync never
    /// carries it. Written again on a restore, the marker would need a part id of its own: a row
    /// the other machine lacks, which capture would push as new.
    fn restores_fork_links(&self) -> bool {
        false
    }

    fn program(&self) -> &'static str {
        self.flavour.binary()
    }

    fn home_vars(&self) -> &'static [&'static str] {
        match self.flavour {
            Flavour::Opencode => &[
                "XDG_CONFIG_HOME",
                "XDG_DATA_HOME",
                "OPENCODE_DB",
                "OPENCODE_DISABLE_CHANNEL_DB",
            ],
            // Kilo takes the config directory from `KILO_CONFIG_DIR` first (see
            // `Flavour::config_dir`), so the daemon that launchd starts has to hand it over too.
            Flavour::Kilo => &[
                "KILO_CONFIG_DIR",
                "XDG_CONFIG_HOME",
                "XDG_DATA_HOME",
                "KILO_DB",
                "KILO_DISABLE_CHANNEL_DB",
            ],
        }
    }

    /// Install the hooks on the current active user's machine
    async fn install_hooks(&self) -> Result<PathBuf, InstallHookError> {
        const PLUGIN_NAME: &str = "atuin.ts";
        // Kilo is 1.x-shaped, so it loads 1.x plugins only; opencode 2's plugin shape would
        // simply go unloaded there. (Kilo scans `{plugin,plugins}/*.{ts,js}` either way, so the
        // `plugins` directory is the one atuin installs into for both.)
        let plugin = match self.flavour {
            Flavour::Opencode => plugin_flavour(
                self.flavour,
                rehydrate::major_version(Path::new("opencode"), self.flavour).await.ok(),
            ),
            Flavour::Kilo => plugin_flavour(self.flavour, None),
        };
        let plugin = plugin_with_executable(&plugin, TS_ATUIN_LINE, launch_path().await?)?;

        let plugins_dir = self.flavour.config_dir().joined("plugins");

        tokio::fs::create_dir_all(&plugins_dir).await?;

        let extension_path = plugins_dir.joined(PLUGIN_NAME);

        let already_installed = tokio::fs::read_to_string(&extension_path)
            .await
            .is_ok_and(|existing| existing == plugin.as_str());

        if already_installed {
            return Err(InstallHookError::AlreadyInstalled);
        }

        tokio::fs::write(&extension_path, &plugin).await?;

        Ok(extension_path)
    }

    async fn has_hooks(&self) -> Result<bool, InstallHookError> {
        super::own_file_exists(&self.flavour.config_dir().joined("plugins").joined("atuin.ts")).await
    }

    async fn uninstall_hooks(&self) -> Result<bool, InstallHookError> {
        super::remove_own_file(&self.flavour.config_dir().joined("plugins").joined("atuin.ts")).await
    }

    /// `opencode --session <id>`. opencode looks the id up in its one global database and serves
    /// the session from the directory recorded for it, but the TUI starts in the directory it is
    /// run from (its project's session list, config and the new sessions it creates), so the
    /// session's own is the one to be in.
    fn resume_plan(&self, target: &ResumeTarget) -> Result<ResumePlan, ResumeError> {
        let args = ["--session".to_owned(), target.id.clone()];
        Ok(resume::plan(target, CwdRequirement::Preferred, self.program(), args))
    }

    async fn locate(&self, id: &str) -> Option<PathBuf> {
        session::locate(&session::default_db(self.flavour)?, id).await
    }

    async fn rehydrate(
        &self,
        session: &RehydrateSession,
    ) -> Result<Rehydrated, RehydrateError> {
        rehydrate::rehydrate(session, self.flavour)
            .await
            .map(Rehydrated::at)
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

    /// Each flavour installs the plugin its loader takes: Kilo is 1.x-shaped (Kilo never ships
    /// opencode 2's `session import`), so a Kilo install never takes opencode 2's plugin shape,
    /// however high its version major is (Kilo's is 7).
    #[rstest]
    #[case(Flavour::Opencode)]
    #[case(Flavour::Kilo)]
    fn each_flavour_names_itself_and_its_binary(#[case] flavour: Flavour) {
        let (name, program, display) = match flavour {
            Flavour::Opencode => ("opencode", "opencode", "opencode"),
            Flavour::Kilo => ("kilo", "kilo", "Kilo Code"),
        };
        assert_eq!(Opencode { flavour }.name(), name);
        assert_eq!(Opencode { flavour }.program(), program);
        assert_eq!(Opencode { flavour }.display_name(), display);
    }

    /// The plugin shape each flavour gets: opencode 2 a default export; opencode 1.x and Kilo
    /// (1.x-shaped, whatever its `kilo --version` prints) a named export.
    #[rstest]
    fn each_flavour_gets_the_plugin_it_loads() {
        assert_eq!(plugin_flavour(Flavour::Opencode, Some(1)), PLUGIN_1);
        assert_eq!(plugin_flavour(Flavour::Opencode, Some(2)), PLUGIN_2);
        let kilo = plugin_flavour(Flavour::Kilo, Some(7));
        assert!(
            kilo.contains("const ATUIN_AUTHOR = \"kilo\";"),
            "Kilo plugin must record under the kilo author"
        );
        assert!(
            !kilo.contains("const ATUIN_AUTHOR = \"opencode\";"),
            "Kilo plugin must not record under the opencode author"
        );
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
