use std::process::Command;
use std::{collections::HashMap, path::PathBuf};

use atuin_client::settings::Settings;
use eyre::Result;
use rustix::path::Arg;
use serde::{Deserialize, Serialize};

use sysinfo::{get_current_pid, System};

#[derive(Debug, Serialize, Deserialize)]
struct ShellInfo {
    pub name: String,

    // Detect some shell plugins that the user has installed.
    // I'm just going to start with preexec/blesh
    pub plugins: Vec<String>,
}

impl ShellInfo {
    pub fn plugins(shell: &str) -> Vec<String> {
        // consider a different detection approach if there are plugins
        // that don't set env vars

        // HACK ALERT!
        // Many of the env vars we need to detect are not exported :(
        // So, we're going to run `env` in a subshell and parse the output
        // There's a chance this won't work, so it should not be fatal.
        //
        // Every shell we support handles `shell -c 'command'`
        let cmd = Command::new(shell)
            .args(["-c", "env"])
            .output()
            .map_or(String::new(), |v| {
                let out = v.stdout;

                String::from(out.as_str().unwrap_or(""))
            });

        let map = HashMap::from([
            ("BLE_ATTACHED", "blesh"),
            ("bash_preexec_imported", "bash-preexec"),
        ]);

        map.into_iter()
            .filter_map(|(env, plugin)| {
                if cmd.contains(env) {
                    return Some(plugin.to_string());
                }

                None
            })
            .collect()
    }

    pub fn new() -> Self {
        let sys = System::new_all();

        let process = sys
            .process(get_current_pid().expect("Failed to get current PID"))
            .expect("Process with current pid does not exist");

        let parent = sys
            .process(process.parent().expect("Atuin running with no parent!"))
            .expect("Process with parent pid does not exist");

        let shell = parent.name().trim().to_lowercase();
        let shell = shell.strip_prefix('-').unwrap_or(&shell);
        let name = shell.to_string();

        let plugins = ShellInfo::plugins(name.as_str());

        Self { name, plugins }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct OsInfo {
    pub name: String,

    pub arch: String,

    pub version: String,
}

impl OsInfo {
    pub fn new() -> Self {
        Self {
            name: System::name().unwrap_or_else(|| "unknown".to_string()),
            arch: System::cpu_arch().unwrap_or_else(|| "unknown".to_string()),
            version: System::os_version().unwrap_or_else(|| "unknown".to_string()),
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
}

impl AtuinInfo {
    pub fn new(settings: &Settings) -> Self {
        let session_path = settings.session_path.as_str();
        let logged_in = PathBuf::from(session_path).exists();

        let sync = if logged_in {
            Some(SyncInfo::new(settings))
        } else {
            None
        };

        Self {
            version: crate::VERSION.to_string(),
            sync,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct DoctorDump {
    pub atuin: AtuinInfo,
    pub shell: ShellInfo,
    pub os: OsInfo,
}

impl DoctorDump {
    pub fn new(settings: &Settings) -> Self {
        Self {
            atuin: AtuinInfo::new(settings),
            shell: ShellInfo::new(),
            os: OsInfo::new(),
        }
    }
}

pub fn run(settings: &Settings) -> Result<()> {
    let dump = DoctorDump::new(settings);

    let dump = serde_yaml::to_string(&dump)?;
    println!("{dump}");

    Ok(())
}
