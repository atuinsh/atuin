use std::process::Command;
use std::{env, path::PathBuf, str::FromStr};

use atuin_client::database::Sqlite;
use atuin_client::settings::Settings;
use atuin_common::shell::{shell_name, Shell};
use colored::Colorize;
use eyre::Result;
use serde::{Deserialize, Serialize};

use sysinfo::{get_current_pid, Disks, System};

#[derive(Debug, Serialize, Deserialize)]
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
    fn shellvar_exists(shell: &str, var: &str) -> bool {
        let cmd = Command::new(shell)
            .args([
                "-ic",
                format!("[ -z ${var} ] || echo ATUIN_DOCTOR_ENV_FOUND").as_str(),
            ])
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
            env::var("ATUIN_PREEXEC_BACKEND")
                .ok()
                .filter(|value| !value.is_empty())
                .and_then(|atuin_preexec_backend| {
                    atuin_preexec_backend.rfind(':').and_then(|pos_colon| {
                        u32::from_str(&atuin_preexec_backend[..pos_colon])
                            .ok()
                            .is_some_and(|preexec_shlvl| {
                                env::var("SHLVL")
                                    .ok()
                                    .and_then(|shlvl| u32::from_str(&shlvl).ok())
                                    .is_some_and(|shlvl| shlvl == preexec_shlvl)
                            })
                            .then(|| atuin_preexec_backend[pos_colon + 1..].to_string())
                    })
                })
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

        let plugin_list: [(
            &str,
            PluginShellType,
            PluginProbeType,
            Option<PluginValidator>,
        ); 3] = [
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

    pub fn new() -> Self {
        // TODO: rework to use atuin_common::Shell

        let sys = System::new_all();

        let process = sys
            .process(get_current_pid().expect("Failed to get current PID"))
            .expect("Process with current pid does not exist");

        let parent = sys
            .process(process.parent().expect("Atuin running with no parent!"))
            .expect("Process with parent pid does not exist");

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

#[derive(Debug, Serialize, Deserialize)]
struct DiskInfo {
    pub name: String,
    pub filesystem: String,
}

#[derive(Debug, Serialize, Deserialize)]
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

#[derive(Debug, Serialize, Deserialize)]
struct SyncInfo {
    /// Whether the main Atuin sync server is in use
    /// I'm just calling it Atuin Cloud for lack of a better name atm
    pub cloud: bool,
    pub records: bool,
    pub auto_sync: bool,

    pub last_sync: String,
}

impl SyncInfo {
    pub fn new(settings: &Settings) -> Self {
        Self {
            cloud: settings.sync_address == "https://api.atuin.sh",
            auto_sync: settings.auto_sync,
            records: settings.sync.records,
            last_sync: Settings::last_sync().map_or("no last sync".to_string(), |v| v.to_string()),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct AtuinInfo {
    pub version: String,

    /// Whether the main Atuin sync server is in use
    /// I'm just calling it Atuin Cloud for lack of a better name atm
    pub sync: Option<SyncInfo>,

    pub sqlite_version: String,
}

impl AtuinInfo {
    pub async fn new(settings: &Settings) -> Self {
        let session_path = settings.session_path.as_str();
        let logged_in = PathBuf::from(session_path).exists();

        let sync = if logged_in {
            Some(SyncInfo::new(settings))
        } else {
            None
        };

        let sqlite_version = match Sqlite::new("sqlite::memory:", 0.1).await {
            Ok(db) => db
                .sqlite_version()
                .await
                .unwrap_or_else(|_| "unknown".to_string()),
            Err(_) => "error".to_string(),
        };

        Self {
            version: crate::VERSION.to_string(),
            sync,
            sqlite_version,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct DoctorDump {
    pub atuin: AtuinInfo,
    pub shell: ShellInfo,
    pub system: SystemInfo,
}

impl DoctorDump {
    pub async fn new(settings: &Settings) -> Self {
        Self {
            atuin: AtuinInfo::new(settings).await,
            shell: ShellInfo::new(),
            system: SystemInfo::new(),
        }
    }
}

fn checks(info: &DoctorDump) {
    println!(); // spacing
                //
    let zfs_error = "[Filesystem] ZFS is known to have some issues with SQLite. Atuin uses SQLite heavily. If you are having poor performance, there are some workarounds here: https://github.com/atuinsh/atuin/issues/952".bold().red();
    let bash_plugin_error = "[Shell] If you are using Bash, Atuin requires that either bash-preexec or ble.sh be installed. An older ble.sh may not be detected. so ignore this if you have it set up! Read more here: https://docs.atuin.sh/guide/installation/#bash".bold().red();
    let blesh_integration_error = "[Shell] Atuin and ble.sh seem to be loaded in the session, but the integration does not seem to be working. Please check the setup in .bashrc.".bold().red();

    // ZFS: https://github.com/atuinsh/atuin/issues/952
    if info.system.disks.iter().any(|d| d.filesystem == "zfs") {
        println!("{zfs_error}");
    }

    // Shell
    if info.shell.name == "bash" {
        if !info
            .shell
            .plugins
            .iter()
            .any(|p| p == "blesh" || p == "bash-preexec")
        {
            println!("{bash_plugin_error}");
        }

        if info.shell.plugins.iter().any(|plugin| plugin == "atuin")
            && info.shell.plugins.iter().any(|plugin| plugin == "blesh")
            && info.shell.preexec.as_ref().is_some_and(|val| val == "none")
        {
            println!("{blesh_integration_error}");
        }
    }
}

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
