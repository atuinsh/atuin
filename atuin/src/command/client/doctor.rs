use std::path::PathBuf;

use atuin_client::{encryption, record::sqlite_store::SqliteStore, settings::Settings};
use atuin_config::store::AliasStore;
use clap::{Parser, ValueEnum};
use eyre::{Result, WrapErr};
use serde::{Deserialize, Serialize};

use sysinfo::{get_current_pid, System};

#[derive(Debug, Serialize, Deserialize)]
struct ShellInfo {
    pub name: String,
}

impl ShellInfo {
    pub fn new() -> Self {
        let sys = System::new_all();

        let process = sys
            .process(get_current_pid().expect("Failed to get current PID"))
            .expect("Process with current pid does not exist");

        let parent = sys
            .process(process.parent().expect("Atuin running with no parent!"))
            .expect("Process with parent pid does not exist");

        let shell = parent.name().trim().to_lowercase();
        let shell = shell.strip_prefix("-").unwrap_or(&shell);
        let name = shell.to_string();

        Self { name }
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
            name: System::name().unwrap_or("unknown".to_string()),
            arch: System::cpu_arch().unwrap_or("unknown".to_string()),
            version: System::os_version().unwrap_or("unknown".to_string()),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct SyncInfo {
    /// Whether the main Atuin sync server is in use
    /// I'm just calling it Atuin Cloud for lack of a better name atm
    pub cloud: bool,
    pub registered: bool,
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
            registered: !settings.session_token.is_empty(),
            last_sync: Settings::last_sync().map_or("no last sync".to_string(), |v| v.to_string()),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct AtuinInfo {
    pub version: String,

    /// Whether the main Atuin sync server is in use
    /// I'm just calling it Atuin Cloud for lack of a better name atm
    pub sync: SyncInfo,
}

impl AtuinInfo {
    pub fn new(settings: &Settings) -> Self {
        Self {
            version: crate::VERSION.to_string(),
            sync: SyncInfo::new(settings),
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

#[derive(Parser, Debug)]
pub struct Cmd {}

impl Cmd {
    pub fn run(&self, settings: &Settings) -> Result<()> {
        let dump = DoctorDump::new(settings);

        let dump = serde_yaml::to_string(&dump)?;
        println!("{}", dump);

        Ok(())
    }
}
