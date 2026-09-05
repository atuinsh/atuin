use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::common::FreshEnv;
use crate::pty::PtyShell;

pub const PROMPT: &str = "E2E_PROMPT>";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShellConfig {
    shell: String,
    #[serde(default)]
    args: Vec<String>,
    rc: PathBuf,
    script: String,
    #[serde(default = "enter")]
    pub multiline_accept: String,
    #[serde(default)]
    required_files: BTreeMap<String, String>,
}

fn enter() -> String {
    "\r".into()
}

pub struct Shell {
    // Drop the shell before deleting its home.
    pub pty: PtyShell,
    pub env: FreshEnv,
    pub config: ShellConfig,
}

impl Shell {
    pub fn start(path: &Path, settings: Option<&str>) -> Option<Self> {
        let config: ShellConfig = toml_edit::de::from_str(&fs::read_to_string(path).unwrap())
            .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let override_var = format!("ATUIN_E2E_{}", config.shell.to_uppercase());
        let executable = if let Some(path) = std::env::var_os(&override_var) {
            let path = PathBuf::from(path);
            assert!(path.is_file(), "{override_var} is not a file: {}", path.display());
            Some(path)
        } else {
            std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
                .map(|dir| dir.join(&config.shell))
                .find(|path| path.is_file())
        };
        let Some(executable) = executable else {
            missing(path, &config.shell);
            return None;
        };
        let mut vars = BTreeMap::new();
        for (name, default) in &config.required_files {
            let value = std::env::var(name).unwrap_or_else(|_| {
                default.replace("$HOME", &std::env::var("HOME").unwrap_or_default())
            });
            if !Path::new(&value).is_file() {
                missing(path, &format!("{name}={value}"));
                return None;
            }
            vars.insert(name.clone(), value);
        }
        let env = FreshEnv::new();
        if let Some(settings) = settings {
            env.write_config(settings);
        }
        let rc = env.home().join(&config.rc);
        fs::create_dir_all(rc.parent().unwrap()).unwrap();
        fs::write(rc, &config.script).unwrap();
        let pty = PtyShell::spawn(&executable, &config.args, &env, &vars);
        pty.wait_for_prompt();
        Some(Self { pty, env, config })
    }
}

fn missing(config: &Path, dependency: &str) {
    assert!(
        std::env::var_os("ATUIN_E2E_REQUIRE_SHELLS").is_none(),
        "{}: missing {dependency}",
        config.display()
    );
    eprintln!("skipping {}: missing {dependency}", config.display());
}
