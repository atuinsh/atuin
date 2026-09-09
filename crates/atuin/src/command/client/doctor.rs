use std::env;
use std::path::PathBuf;
use std::process::Command;
use std::str::FromStr;
use std::time::Duration;

use atuin_client::database::Sqlite;
use atuin_client::settings::Settings;
use atuin_common::path::PathExt;
use atuin_common::shell::{Shell, shell_name};
use colored::Colorize;
use eyre::Result;
use serde::Serialize;
use sysinfo::{Disks, System, get_current_pid};
use tracing::instrument;

#[derive(Debug, Serialize)]
struct ShellInfo {
    pub name: String,

    // best-effort, not supported on all OSes
    pub default: String,

    // Detect some shell plugins that the user has installed.
    // I'm just going to start with preexec/blesh
    pub plugins: Vec<String>,

    // The preexec framework used in the current session, if Atuin is loaded.
    pub preexec: Option<String>,
}

impl ShellInfo {
    // HACK ALERT!
    // Many of the shell vars we need to detect are not exported :(
    // So, we're going to run a interactive session and directly check the
    // variable.  There's a chance this won't work, so it should not be fatal.
    //
    // Every shell we support handles `shell -ic 'command'`
    #[must_use]
    fn shellvar_exists(shell: &str, var: &str) -> bool {
        let cmd = Command::new(shell)
            .args(["-ic", format!("[ -z ${var} ] || echo ATUIN_DOCTOR_ENV_FOUND").as_str()])
            .output()
            .map_or(String::new(), |v| {
                let out = v.stdout;
                String::from_utf8(out).unwrap_or_default()
            });

        cmd.contains("ATUIN_DOCTOR_ENV_FOUND")
    }

    fn detect_preexec_framework(shell: &str) -> Option<String> {
        if env::var("ATUIN_SESSION").ok().is_none() {
            None
        } else if shell.starts_with("bash") || shell == "sh" {
            env::var("ATUIN_PREEXEC_BACKEND").ok().filter(|value| !value.is_empty()).and_then(
                |atuin_preexec_backend| {
                    atuin_preexec_backend.rfind(':').and_then(|pos_colon| {
                        u32::from_str(&atuin_preexec_backend[..pos_colon])
                            .is_ok_and(|preexec_shlvl| {
                                env::var("SHLVL")
                                    .ok()
                                    .and_then(|shlvl| u32::from_str(&shlvl).ok())
                                    .is_some_and(|shlvl| shlvl == preexec_shlvl)
                            })
                            .then(|| atuin_preexec_backend[pos_colon + 1..].to_string())
                    })
                },
            )
        } else {
            Some("built-in".to_string())
        }
    }

    fn validate_plugin_blesh(
        _shell: &str,
        shell_process: &sysinfo::Process,
        ble_session_id: &str,
    ) -> Option<String> {
        ble_session_id
            .split('/')
            .nth(1)
            .and_then(|field| u32::from_str(field).ok())
            .filter(|&blesh_pid| blesh_pid == shell_process.pid().as_u32())
            .map(|_| "blesh".to_string())
    }

    #[must_use]
    pub fn plugins(shell: &str, shell_process: &sysinfo::Process) -> Vec<String> {
        // consider a different detection approach if there are plugins
        // that don't set shell vars

        enum PluginShellType {
            Any,
            Bash,

            // Note: these are currently unused
            #[allow(dead_code)]
            Zsh,
            #[allow(dead_code)]
            Fish,
            #[allow(dead_code)]
            Nushell,
            #[allow(dead_code)]
            Xonsh,
        }

        enum PluginProbeType {
            EnvironmentVariable(&'static str),
            InteractiveShellVariable(&'static str),
        }

        type PluginValidator = fn(&str, &sysinfo::Process, &str) -> Option<String>;

        let plugin_list: [(&str, PluginShellType, PluginProbeType, Option<PluginValidator>); 3] = [
            (
                "atuin",
                PluginShellType::Any,
                PluginProbeType::EnvironmentVariable("ATUIN_SESSION"),
                None,
            ),
            (
                "blesh",
                PluginShellType::Bash,
                PluginProbeType::EnvironmentVariable("BLE_SESSION_ID"),
                Some(Self::validate_plugin_blesh),
            ),
            (
                "bash-preexec",
                PluginShellType::Bash,
                PluginProbeType::InteractiveShellVariable("bash_preexec_imported"),
                None,
            ),
        ];

        plugin_list
            .into_iter()
            .filter(|(_, shell_type, _, _)| match shell_type {
                PluginShellType::Any => true,
                PluginShellType::Bash => shell.starts_with("bash") || shell == "sh",
                PluginShellType::Zsh => shell.starts_with("zsh"),
                PluginShellType::Fish => shell.starts_with("fish"),
                PluginShellType::Nushell => shell.starts_with("nu"),
                PluginShellType::Xonsh => shell.starts_with("xonsh"),
            })
            .filter_map(|(plugin, _, probe_type, validator)| -> Option<String> {
                match probe_type {
                    PluginProbeType::EnvironmentVariable(env) => {
                        env::var(env).ok().filter(|value| !value.is_empty())
                    }
                    PluginProbeType::InteractiveShellVariable(shellvar) => {
                        ShellInfo::shellvar_exists(shell, shellvar).then_some(String::default())
                    }
                }
                .and_then(|value| {
                    validator.map_or_else(
                        || Some(plugin.to_string()),
                        |validator| validator(shell, shell_process, &value),
                    )
                })
            })
            .collect()
    }

    fn unknown() -> Self {
        let name = Shell::Unknown.to_string();
        let default = Shell::default_shell().unwrap_or(Shell::Unknown).to_string();
        let preexec = Self::detect_preexec_framework(name.as_str());

        Self {
            name,
            default,
            plugins: Vec::new(),
            preexec,
        }
    }

    pub fn new() -> Self {
        // TODO: rework to use atuin_common::Shell

        let sys = System::new_all();

        let Some(process) = get_current_pid().ok().and_then(|pid| sys.process(pid)) else {
            return Self::unknown();
        };

        let Some(parent) = process.parent().and_then(|pid| sys.process(pid)) else {
            return Self::unknown();
        };

        let name = shell_name(Some(parent));

        let plugins = ShellInfo::plugins(name.as_str(), parent);

        let default = Shell::default_shell().unwrap_or(Shell::Unknown).to_string();

        let preexec = Self::detect_preexec_framework(name.as_str());

        Self {
            name,
            default,
            plugins,
            preexec,
        }
    }
}

#[derive(Debug, Serialize)]
struct DiskInfo {
    pub name: String,
    pub filesystem: String,
}

#[derive(Debug, Serialize)]
struct SystemInfo {
    pub os: String,

    pub arch: String,

    pub version: String,
    pub disks: Vec<DiskInfo>,
}

impl SystemInfo {
    pub fn new() -> Self {
        let disks = Disks::new_with_refreshed_list();
        let disks = disks
            .list()
            .iter()
            .map(|d| DiskInfo {
                name: d.name().to_os_string().into_string().unwrap(),
                filesystem: d.file_system().to_os_string().into_string().unwrap(),
            })
            .collect();

        Self {
            os: System::name().unwrap_or_else(|| "unknown".to_string()),
            arch: System::cpu_arch().unwrap_or_else(|| "unknown".to_string()),
            version: System::os_version().unwrap_or_else(|| "unknown".to_string()),
            disks,
        }
    }
}

#[derive(Debug, Serialize)]
struct SyncInfo {
    pub auth_state: String,
    pub auto_sync: bool,

    pub last_sync: String,
}

impl SyncInfo {
    pub async fn new(settings: &Settings) -> Self {
        // Build auth state description from raw token state without calling
        // resolve_sync_auth(), which has side effects (token migration cleanup)
        // that a diagnostic command should not trigger.
        let meta = Settings::meta_store().await.ok();
        let has_hub_token = match &meta {
            Some(m) => {
                m.hub_session_token().await.ok().flatten().is_some_and(|t| t.starts_with("atapi_"))
            }
            None => false,
        };
        let has_cli_token = match &meta {
            Some(m) => m.session_token().await.ok().flatten().is_some(),
            None => false,
        };

        let auth_state = if has_hub_token {
            "Hub (authenticated)".into()
        } else if settings.is_hub_sync() && has_cli_token {
            "Hub (legacy token \u{2014} run 'atuin login' to upgrade)".into()
        } else if !settings.is_hub_sync() && has_cli_token {
            "Self-hosted (authenticated)".into()
        } else {
            "Not authenticated".into()
        };

        Self {
            auth_state,
            auto_sync: settings.auto_sync,
            last_sync: Settings::last_sync()
                .await
                .map_or_else(|_| "no last sync".to_string(), |v| v.to_string()),
        }
    }
}

#[derive(Debug)]
struct SettingPaths {
    db: PathBuf,
    record_store: PathBuf,
    key: PathBuf,
}

impl SettingPaths {
    pub fn new(settings: &Settings) -> Self {
        Self {
            db: settings.db_path.clone(),
            record_store: settings.record_store_path.clone(),
            key: settings.key_path.clone(),
        }
    }

    pub fn verify(&self) {
        let paths = vec![
            ("ATUIN_DB_PATH", &self.db),
            ("ATUIN_RECORD_STORE", &self.record_store),
            ("ATUIN_KEY", &self.key),
        ];

        for (path_env_var, path) in paths {
            if path.as_path().is_dangling_symlink() {
                eprintln!(
                    "{} (${path_env_var}) is a broken symlink. This may cause issues with Atuin.",
                    path.display()
                );
            }
        }
    }
}

#[derive(Debug, Serialize)]
struct AtuinInfo {
    pub version: String,
    pub commit: String,

    /// Whether the main Atuin sync server is in use
    /// I'm just calling it Atuin Cloud for lack of a better name atm
    pub sync: Option<SyncInfo>,

    pub sqlite_version: String,

    #[serde(skip)] // probably unnecessary to expose this
    pub setting_paths: SettingPaths,

    pub daemon_enabled: bool,
}

impl AtuinInfo {
    pub async fn new(settings: &Settings) -> Self {
        let logged_in = settings.logged_in().await.unwrap_or(false);

        let sync = if logged_in {
            Some(SyncInfo::new(settings).await)
        } else {
            None
        };

        let sqlite_version = match Sqlite::in_memory(Duration::from_millis(100)).await {
            Ok(db) => {
                db.sqlite_version().await.map_or_else(|_| "unknown".to_string(), |v| v.to_string())
            }
            Err(_) => "error".to_string(),
        };

        Self {
            version: crate::VERSION.to_string(),
            commit: crate::SHA.to_string(),
            sync,
            sqlite_version,
            setting_paths: SettingPaths::new(settings),
            daemon_enabled: cfg!(feature = "daemon") && settings.daemon.enabled,
        }
    }
}

#[derive(Debug, Serialize)]
struct OutputCaptureInfo {
    /// One of: active, disabled, daemon-disabled, daemon-not-running, daemon-outdated, error,
    /// unsupported.
    status: String,
    detail: Option<String>,
    store: Option<OutputCaptureStoreInfo>,
}

#[derive(Debug, Serialize)]
struct OutputCaptureStoreInfo {
    stored_captures: u64,
    disk_bytes_flushed: u64,
    oldest_capture: Option<String>,
    newest_capture: Option<String>,
    store_path: String,
    schema: String,
}

/// Render a unix-millisecond timestamp as an RFC3339 UTC string, or `None` if it is out of range.
fn format_capture_time(unix_ms: u64) -> Option<String> {
    use atuin_common::time::OffsetDateTimeExt;
    use time::OffsetDateTime;
    use time::format_description::well_known::Rfc3339;
    let nanos = i128::from(unix_ms).checked_mul(1_000_000)?;
    OffsetDateTime::from_unix_nanos(nanos).ok()?.format(&Rfc3339).ok()
}

impl OutputCaptureInfo {
    fn status(status: &str, detail: &str) -> Self {
        Self { status: status.to_string(), detail: Some(detail.to_string()), store: None }
    }

    #[cfg(feature = "daemon")]
    async fn new(settings: &Settings) -> Self {
        use super::daemon::{OutputCaptureReport, output_capture_report};

        if !settings.daemon.enabled {
            return Self::status(
                "daemon-disabled",
                "the daemon is disabled in settings; enable it to capture command output",
            );
        }

        match output_capture_report(settings).await {
            OutputCaptureReport::Active(store) => Self {
                status: "active".to_string(),
                detail: None,
                store: Some(OutputCaptureStoreInfo {
                    stored_captures: store.stored_captures,
                    disk_bytes_flushed: store.disk_bytes,
                    oldest_capture: store.oldest_capture_unix_ms.and_then(format_capture_time),
                    newest_capture: store.newest_capture_unix_ms.and_then(format_capture_time),
                    store_path: store.store_path,
                    schema: store.schema,
                }),
            },
            OutputCaptureReport::Disabled => Self::status(
                "disabled",
                "the daemon could not open its capture store; command output is not being saved",
            ),
            OutputCaptureReport::NeedsRestart(reason) => {
                Self { status: "daemon-outdated".to_string(), detail: Some(reason), store: None }
            }
            OutputCaptureReport::NotRunning => Self::status(
                "daemon-not-running",
                "the daemon is not running; start it with `atuin daemon start`",
            ),
            OutputCaptureReport::Error(msg) => {
                Self { status: "error".to_string(), detail: Some(msg), store: None }
            }
        }
    }

    #[cfg(not(feature = "daemon"))]
    async fn new(_settings: &Settings) -> Self {
        Self::status("unsupported", "this atuin build was compiled without daemon support")
    }
}

#[derive(Debug, Serialize)]
struct DoctorDump {
    pub atuin: AtuinInfo,
    pub shell: ShellInfo,
    pub system: SystemInfo,
    pub output_capture: OutputCaptureInfo,
}

impl DoctorDump {
    pub async fn new(settings: &Settings) -> Self {
        Self {
            atuin: AtuinInfo::new(settings).await,
            shell: ShellInfo::new(),
            system: SystemInfo::new(),
            output_capture: OutputCaptureInfo::new(settings).await,
        }
    }
}

fn checks(info: &DoctorDump) {
    println!(); // spacing
    //
    let zfs_error = "[Filesystem] ZFS is known to have some issues with SQLite. Atuin uses SQLite heavily. If you are having poor performance, there are some workarounds here: https://github.com/atuinsh/atuin/issues/952".bold().red();
    let bash_plugin_error = format!(
        "[Shell] If you are using Bash, Atuin requires that either bash-preexec or ble.sh (>= \
         0.4) be installed. An older ble.sh may not be detected. so ignore this if you have \
         ble.sh >= 0.4 set up! Read more here: {}",
        atuin_common::docs::url("guide/installation/#installing-the-shell-plugin")
    )
    .bold()
    .red();
    let blesh_integration_error = "[Shell] Atuin and ble.sh seem to be loaded in the session, but \
                                   the integration does not seem to be working. Please check the \
                                   setup in .bashrc."
        .bold()
        .red();
    let openbsd_warning = "[System] OpenBSD is not officially supported.".bold().red();

    if cfg!(target_os = "openbsd") {
        println!("{openbsd_warning}");
    }

    // ZFS: https://github.com/atuinsh/atuin/issues/952
    if info.system.disks.iter().any(|d| d.filesystem == "zfs") {
        println!("{zfs_error}");
    }

    info.atuin.setting_paths.verify();

    // Shell
    if info.shell.name == "bash" {
        if !info.shell.plugins.iter().any(|p| p == "blesh" || p == "bash-preexec") {
            println!("{bash_plugin_error}");
        }

        if info.shell.plugins.iter().any(|plugin| plugin == "atuin")
            && info.shell.plugins.iter().any(|plugin| plugin == "blesh")
            && info.shell.preexec.as_ref().is_some_and(|val| val == "none")
        {
            println!("{blesh_integration_error}");
        }
    }

    if info.output_capture.status == "disabled" {
        println!(
            "{}",
            "[Output capture] The daemon could not open its capture store, so command output is \
             not being saved. Check the daemon logs and the permissions on the output-capture \
             directory."
                .bold()
                .red()
        );
    }
}

#[instrument(level = "trace", skip_all, err)]
pub async fn run(settings: &Settings) -> Result<()> {
    println!("{}", "Atuin Doctor".bold());
    println!("Checking for diagnostics");
    let dump = DoctorDump::new(settings).await;

    checks(&dump);

    let dump = serde_json::to_string_pretty(&dump)?;

    println!("\nPlease include the output below with any bug reports or issues\n");
    println!("{dump}");

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_capture_time_renders_rfc3339_utc() {
        // 2021-01-01T00:00:00Z is 1_609_459_200_000 ms since the epoch.
        assert_eq!(
            format_capture_time(1_609_459_200_000).as_deref(),
            Some("2021-01-01T00:00:00Z"),
        );
    }
}
