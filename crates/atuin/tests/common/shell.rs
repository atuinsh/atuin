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
    pub shell: String,
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
        let setup = Setup::find(path)?;
        let env = FreshEnv::new();
        if let Some(settings) = settings {
            env.write_config(settings);
        }
        // Finish migrations before background shell hooks can open the databases.
        env.run(&["store", "status"]);
        let (pty, config) = setup.spawn(&env, BTreeMap::new(), "");
        Some(Self { pty, env, config })
    }
}

/// A shell setup whose shell, and every file it needs, is installed.
pub struct Setup {
    path: PathBuf,
    config: ShellConfig,
    executable: PathBuf,
    /// Where each file it needs is.
    files: BTreeMap<String, String>,
}

impl Setup {
    /// The shell setup at `path`; `None` (or a failure, where they are required) when its shell
    /// or a file it needs isn't installed. Before anything is set up for it, so a setup skipped
    /// costs nothing.
    pub fn find(path: &Path) -> Option<Self> {
        let config: ShellConfig = toml_edit::de::from_str(&fs::read_to_string(path).unwrap())
            .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let Some(executable) = find_shell(&config.shell) else {
            missing(path, &config.shell);
            return None;
        };
        let mut files = BTreeMap::new();
        for (name, default) in &config.required_files {
            let value = std::env::var(name).unwrap_or_else(|_| {
                default.replace("$HOME", &std::env::var("HOME").unwrap_or_default())
            });
            if !Path::new(&value).is_file() {
                missing(path, &format!("{name}={value}"));
                return None;
            }
            files.insert(name.clone(), value);
        }
        Some(Self {
            path: path.to_owned(),
            config,
            executable,
            files,
        })
    }

    /// The shell started in `env`, with `vars` set (over the files it needs, where they name the
    /// same) and `init_args` passed to `atuin init`, at its prompt.
    pub fn spawn(
        self,
        env: &FreshEnv,
        mut vars: BTreeMap<String, String>,
        init_args: &str,
    ) -> (PtyShell, ShellConfig) {
        let Self {
            path,
            mut config,
            executable,
            files,
        } = self;
        for (name, value) in files {
            vars.entry(name).or_insert(value);
        }
        if !init_args.is_empty() {
            let init = format!("atuin init {}", config.shell);
            assert!(config.script.contains(&init), "{}: no `{init}`", path.display());
            config.script = config.script.replace(&init, &format!("{init} {init_args}"));
        }
        let rc = env.home().join(&config.rc);
        fs::create_dir_all(rc.parent().unwrap()).unwrap();
        fs::write(rc, &config.script).unwrap();
        let pty = PtyShell::spawn(&executable, &config.args, env, &vars);
        pty.wait_for_prompt();
        (pty, config)
    }
}

/// `ATUIN_E2E_<NAME>` if set, otherwise `name` on PATH.
pub fn find_shell(name: &str) -> Option<PathBuf> {
    let override_var = format!("ATUIN_E2E_{}", name.to_uppercase());
    if let Some(path) = std::env::var_os(&override_var) {
        let path = PathBuf::from(path);
        assert!(path.is_file(), "{override_var} is not a file: {}", path.display());
        return Some(path);
    }
    std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|dir| dir.join(name))
        .find(|path| path.is_file())
}

fn missing(config: &Path, dependency: &str) {
    assert!(
        std::env::var_os("ATUIN_E2E_REQUIRE_SHELLS").is_none(),
        "{}: missing {dependency}",
        config.display()
    );
    eprintln!("skipping {}: missing {dependency}", config.display());
}
