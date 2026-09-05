#![allow(dead_code)] // Shared by separate integration-test binaries.

use std::fs::{self, File};
use std::io::{Read, Seek};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output};
use std::time::{Duration, Instant};

use tempfile::TempDir;

pub const TIMEOUT: Duration = Duration::from_secs(30);
pub const SESSION: &str = "b9c063b7b7204f81a50e3e0d51031f01";

/// A temporary home with the built binary on PATH and isolated Atuin data.
pub struct FreshEnv {
    home: TempDir,
}

impl FreshEnv {
    pub fn new() -> Self {
        let home = tempfile::Builder::new().prefix("atuin-e2e-").tempdir_in("/tmp").unwrap();
        let bin = home.path().join("bin");
        fs::create_dir_all(&bin).expect("failed to create bin dir");
        fs::create_dir_all(home.path().join(".config")).unwrap();
        fs::create_dir_all(home.path().join(".local/share")).unwrap();
        fs::create_dir_all(home.path().join(".cache")).unwrap();
        fs::create_dir_all(home.path().join("tmp")).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_atuin"), bin.join("atuin"))
            .expect("failed to symlink atuin into fresh env");
        Self { home }
    }

    pub fn home(&self) -> &Path {
        self.home.path()
    }

    pub fn path_var(&self) -> String {
        let inherited = std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".into());
        format!("{}:{inherited}", self.home.path().join("bin").display())
    }

    /// The environment a fresh interactive session would see. Update checks are
    /// disabled so tests never touch the network.
    pub fn env_vars(&self) -> Vec<(String, String)> {
        let home = self.home.path();
        let mut vars = vec![
            ("HOME".into(), home.display().to_string()),
            ("XDG_CONFIG_HOME".into(), home.join(".config").display().to_string()),
            ("XDG_DATA_HOME".into(), home.join(".local/share").display().to_string()),
            ("XDG_CACHE_HOME".into(), home.join(".cache").display().to_string()),
            ("TMPDIR".into(), home.join("tmp").display().to_string()),
            ("PATH".into(), self.path_var()),
            ("TERM".into(), "xterm-256color".into()),
            ("ATUIN_UPDATE_CHECK".into(), "false".into()),
            ("ATUIN_AUTO_SYNC".into(), "false".into()),
            ("ATUIN_DAEMON__SOCKET_PATH".into(), self.socket().display().to_string()),
            // ble.sh requires a user name.
            ("USER".into(), std::env::var("USER").unwrap_or_else(|_| "e2e".into())),
            ("LOGNAME".into(), std::env::var("LOGNAME").unwrap_or_else(|_| "e2e".into())),
        ];
        for lang in ["LANG", "LC_ALL"] {
            if let Ok(v) = std::env::var(lang) {
                vars.push((lang.into(), v));
            }
        }
        vars
    }

    /// A `Command` for the atuin binary itself, run inside this environment.
    pub fn atuin(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_atuin"));
        cmd.args(args);
        cmd.env_clear();
        cmd.envs(self.env_vars());
        cmd.current_dir(self.home.path());
        cmd
    }

    pub fn socket(&self) -> PathBuf {
        self.home().join("daemon.sock")
    }

    pub fn run(&self, args: &[&str]) -> String {
        output(self.atuin(args))
    }

    pub fn record(&self, command: &str, session: &str, cwd: &Path) {
        let mut start = self.atuin(&["history", "start", "--", command]);
        start.env("ATUIN_SESSION", session).current_dir(cwd);
        let id = output(start);
        self.run(&["history", "end", "--exit", "0", id.trim()]);
    }

    pub fn data_dir(&self) -> PathBuf {
        self.home.path().join(".local/share/atuin")
    }

    /// Write the client config before starting the shell.
    pub fn write_config(&self, contents: &str) {
        let dir = self.home.path().join(".config/atuin");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("config.toml"), contents).unwrap();
    }
}

/// Captures output without risking a full pipe, and reaps children even on failure.
pub struct Process {
    pub child: Child,
    stdout: File,
    stderr: File,
}

impl Process {
    pub fn spawn(mut command: Command) -> Self {
        let stdout = tempfile::tempfile().unwrap();
        let stderr = tempfile::tempfile().unwrap();
        let child = command
            .stdout(stdout.try_clone().unwrap())
            .stderr(stderr.try_clone().unwrap())
            .spawn()
            .expect("failed to spawn command");
        Self {
            child,
            stdout,
            stderr,
        }
    }

    pub fn logs(&self) -> String {
        let mut file = self.stderr.try_clone().unwrap();
        file.rewind().unwrap();
        let mut text = String::new();
        file.read_to_string(&mut text).unwrap();
        text
    }

    pub fn wait(self) -> Output {
        self.try_wait().unwrap_or_else(|error| panic!("{error}"))
    }

    pub fn try_wait(mut self) -> Result<Output, String> {
        let deadline = Instant::now() + TIMEOUT;
        let status = loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                break status;
            }
            if Instant::now() >= deadline {
                return Err(format!("command timed out: {}", self.logs()));
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        self.stdout.rewind().unwrap();
        self.stderr.rewind().unwrap();
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        self.stdout.read_to_end(&mut stdout).unwrap();
        self.stderr.read_to_end(&mut stderr).unwrap();
        Ok(Output {
            status,
            stdout,
            stderr,
        })
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub fn output(command: Command) -> String {
    let out = Process::spawn(command).wait();
    assert!(out.status.success(), "command failed: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).unwrap()
}

pub fn wait_until(what: &str, mut pred: impl FnMut() -> bool) {
    let deadline = Instant::now() + TIMEOUT;
    while !pred() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

pub fn marker() -> String {
    format!("atuin-e2e-{}", atuin_common::utils::uuid_v7().as_simple())
}
