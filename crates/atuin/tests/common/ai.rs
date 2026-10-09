//! Machines running atuin with AI session capture on, and a sync server between them.
//!
//! A [`Machine`] is a temporary home with its own daemon, data and host id, the agents of
//! [`agents`](crate::agents) laid out in it and stand-ins for their executables on its `PATH`.
//! Two machines given the same [`SyncServer`] share one account, so what one captures the other's
//! daemon syncs down, as on two computers.

use std::net::SocketAddr;
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant};

use serde_json::Value;
use tempfile::TempDir;

use crate::agents::{Agent, install_fakes};
use crate::common::{FreshEnv, Process, TIMEOUT, output};

/// An atuin sync server on a port of its own, run on a runtime of its own so the blocking tests
/// that use it never starve it.
pub struct SyncServer {
    pub address: String,
    _dir: TempDir,
    runtime: Option<tokio::runtime::Runtime>,
}

impl SyncServer {
    pub fn start() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let settings = atuin_server::Settings {
            host: "127.0.0.1".to_owned(),
            port: 0,
            path: String::new(),
            open_registration: true,
            max_record_size: atuin_common::units::ByteSize::b(1024 * 1024 * 1024),
            register_webhook_url: None,
            register_webhook_username: String::new(),
            db_settings: atuin_server::db::DbSettings {
                db_uri: format!("sqlite://{}", dir.path().join("server.db").display())
                    .parse()
                    .unwrap(),
            },
            metrics: atuin_server::settings::Metrics::default(),
            fake_version: None,
        };
        let listener = runtime.block_on(tokio::net::TcpListener::bind("127.0.0.1:0")).unwrap();
        let addr: SocketAddr = listener.local_addr().unwrap();
        runtime.spawn(async move {
            atuin_server::launch_with_tcp_listener(settings, listener, std::future::pending())
                .await
                .expect("sync server failed");
        });
        let server = Self {
            address: format!("http://{addr}"),
            _dir: dir,
            runtime: Some(runtime),
        };
        wait("the sync server", || std::net::TcpStream::connect(addr).is_ok());
        server
    }
}

impl Drop for SyncServer {
    fn drop(&mut self) {
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_background();
        }
    }
}

/// Poll `check` until it gives a value, and return that; after [`TIMEOUT`], fail with what it
/// last said was missing.
pub fn wait_for<T>(what: &str, mut check: impl FnMut() -> Result<T, String>) -> T {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        match check() {
            Ok(value) => return value,
            Err(why) => assert!(Instant::now() < deadline, "timed out waiting for {what}: {why}"),
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Poll `ready` until it holds, failing after [`TIMEOUT`].
pub fn wait(what: &str, mut ready: impl FnMut() -> bool) {
    wait_for(what, || {
        if ready() {
            Ok(())
        } else {
            Err(String::new())
        }
    });
}

/// One computer: a home, a daemon capturing sessions, and the agents' executables.
pub struct Machine {
    pub env: FreshEnv,
    /// The agents' stand-ins. Not in the temporary home: that may be on a filesystem that runs
    /// nothing.
    bin: TempDir,
    daemon: Option<Process>,
}

/// Cargo's scratch directory for integration tests, made if it isn't there: a test archive run in
/// a fresh checkout has none.
fn target_tmp() -> &'static Path {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
    std::fs::create_dir_all(dir).unwrap();
    dir
}

/// Where a test keeps what both machines see, like a checkout at the same path on each: a
/// session's working directory has to exist on the machine resuming it for the agent to start
/// there.
pub fn shared_dir() -> TempDir {
    tempfile::Builder::new().prefix("project-").tempdir_in(target_tmp()).unwrap()
}

impl Machine {
    /// A machine on its own, syncing nowhere.
    pub fn new() -> Self {
        Self::with_config("", "capture_sessions = true\n")
    }

    /// A machine syncing nowhere with the settings `top` (top-level keys, then any tables) and
    /// the `[ai]` settings `ai`.
    pub fn with_config(top: &str, ai: &str) -> Self {
        Self::configured(None, top, ai)
    }

    /// A machine with the settings `top` and `[ai]` settings `ai`, syncing through the server at
    /// `sync`, if any, as often as its daemon can.
    fn configured(sync: Option<&str>, top: &str, ai: &str) -> Self {
        let env = FreshEnv::new();
        let bin = tempfile::Builder::new().prefix("bin-").tempdir_in(target_tmp()).unwrap();
        install_fakes(bin.path(), &Agent::ALL);
        for agent in Agent::ALL {
            agent.install(env.home());
        }
        let (auto_sync, address, frequency) = match sync {
            Some(address) => {
                (true, format!("sync_address = \"{address}\"\n"), "sync_frequency = 1\n")
            }
            None => (false, String::new(), ""),
        };
        env.write_config(&format!(
            "auto_sync = {auto_sync}\n{address}{top}[daemon]\nenabled = true\nautostart = \
             false\n{frequency}[ai]\n{ai}"
        ));
        // Finish migrations before the daemon and the commands race for the databases.
        env.run(&["store", "status"]);
        Self {
            env,
            bin,
            daemon: None,
        }
    }

    /// Two machines of one account on `server`, their daemons running.
    pub fn pair(server: &SyncServer) -> (Self, Self) {
        let one = Self::syncing(server, None);
        let key = one.run(&["key", "--base64"]).trim().to_owned();
        let two = Self::syncing(server, Some((&one, &key)));
        (one, two)
    }

    /// A machine syncing through `server`: registering a new account, or logging into `login`'s
    /// with its key. Logged in before its daemon starts, which keeps the key it starts with.
    fn syncing(server: &SyncServer, login: Option<(&Self, &str)>) -> Self {
        let mut machine = Self::configured(Some(&server.address), "", "capture_sessions = true\n");
        match login {
            None => {
                let user = machine.user();
                machine.run(&[
                    "register",
                    "-u",
                    &user,
                    "-e",
                    &format!("{user}@example.com"),
                    "-p",
                    &user,
                ]);
            }
            Some((other, key)) => {
                let user = other.user();
                machine.run(&["login", "-u", &user, "-p", &user, "-k", key]);
            }
        }
        machine.start_daemon();
        machine
    }

    /// The account name this machine registers, and the password: unique to the home.
    fn user(&self) -> String {
        let name = self.env.home().file_name().unwrap().to_string_lossy();
        name.replace(|c: char| !c.is_ascii_alphanumeric(), "")
    }

    pub fn home(&self) -> &Path {
        self.env.home()
    }

    /// This machine's `PATH`: the agents' stand-ins first.
    pub fn path_var(&self) -> String {
        format!("{}:{}", self.bin.path().display(), self.env.path_var())
    }

    /// `atuin args` on this machine, finding the agents' stand-ins on its `PATH`.
    pub fn atuin(&self, args: &[&str]) -> Command {
        let mut command = self.env.atuin(args);
        command.env("PATH", self.path_var());
        // The harness turns automatic sync off for every command; the config decides here.
        command.env_remove("ATUIN_AUTO_SYNC");
        command
    }

    pub fn run(&self, args: &[&str]) -> String {
        output(self.atuin(args))
    }

    pub fn start_daemon(&mut self) {
        assert!(self.daemon.is_none(), "the daemon is already running");
        let mut daemon = self.atuin(&["daemon", "start", "--show-logs"]);
        // `ATUIN_E2E_LOG` sets what the daemons log, as `ATUIN_LOG` does, for a failure to show.
        if let Some(filter) = std::env::var_os("ATUIN_E2E_LOG") {
            daemon.env("ATUIN_LOG", filter);
        }
        let daemon = Process::spawn(daemon);
        let socket = self.env.socket();
        wait("the daemon", || socket.exists());
        self.daemon = Some(daemon);
        // `ai session` commands wait for its first replay of the store, so once one answers, the
        // daemon is capturing.
        self.sessions();
    }

    pub fn stop_daemon(&mut self) {
        let daemon = self.daemon.take().expect("the daemon isn't running");
        self.run(&["daemon", "stop"]);
        let socket = self.env.socket();
        wait("the daemon to stop", || !socket.exists());
        drop(daemon);
    }

    /// The daemon's log so far, for a failure message.
    pub fn logs(&self) -> String {
        self.daemon.as_ref().map(Process::logs).unwrap_or_default()
    }

    /// Every session the daemon has, as `atuin ai session list` gives them.
    pub fn sessions(&self) -> Vec<Value> {
        let list = self.run(&["ai", "session", "list", "--style", "json"]);
        serde_json::from_str::<Value>(&list).unwrap().as_array().unwrap().clone()
    }

    /// The session `id` (the agent's), as `atuin ai session list` gives it.
    pub fn session(&self, id: &str) -> Option<Value> {
        self.sessions().into_iter().find(|s| s["session_id"] == id)
    }

    /// The session `id` and its messages, as `atuin ai session show` gives them.
    pub fn show(&self, id: &str) -> Value {
        let show = self.run(&["ai", "session", "show", id, "--style", "json"]);
        serde_json::from_str(&show).unwrap()
    }

    /// The text of each message of session `id` that says something in conversation, in order.
    pub fn said(&self, id: &str) -> Vec<String> {
        said_in(&self.show(id))
    }

    /// Wait for `check` to give a value, as [`wait_for`], saying on failure what the daemon
    /// logged as well.
    fn wait_for<T>(&self, what: &str, mut check: impl FnMut() -> Result<T, String>) -> T {
        wait_for(what, || check().map_err(|why| format!("{why}\ndaemon log:\n{}", self.logs())))
    }

    /// What session `id` says in conversation, or why there's nothing to say.
    fn current(&self, id: &str) -> Result<Vec<String>, String> {
        match self.session(id) {
            Some(_) => Ok(self.said(id)),
            None => Err(format!("there's no session {id}")),
        }
    }

    /// Wait for session `id` to hold exactly `want` in conversation, failing with what it holds.
    pub fn wait_until_said(&self, id: &str, want: &[String]) {
        self.wait_for(&format!("session {id} to say {want:#?}"), || {
            let said = self.current(id)?;
            if said == want {
                Ok(())
            } else {
                Err(format!("it says {said:#?}"))
            }
        });
    }

    /// Wait for session `id` to hold `want` in conversation, in order, among whatever else.
    pub fn wait_until_holds(&self, id: &str, want: &[String]) {
        self.wait_for(&format!("session {id} to hold {want:#?}"), || {
            let said = self.current(id)?.join("\n");
            if holds_in_order(&said, want) {
                Ok(())
            } else {
                Err(format!("it says {said:?}"))
            }
        });
    }

    /// Wait for a session of `agent` whose parent is session `parent` to appear, and return it.
    /// A new session is listed as soon as its first row is stored, which can be before the row
    /// that names its parent.
    pub fn wait_for_child(&self, agent: Agent, parent: &str) -> Value {
        self.wait_for(&format!("a {agent:?} session under {parent}"), || {
            self.sessions()
                .into_iter()
                .find(|s| s["harness"] == agent.harness() && s["parent"]["session_id"] == parent)
                .ok_or_else(|| "none yet".to_owned())
        })
    }

    /// `atuin ai resume args`, run from `cwd` as a shell there would, without a terminal: never
    /// the picker, which a test can't answer.
    pub fn resume(&self, args: &[&str], cwd: &Path) -> Output {
        let mut command = self.atuin(&[&["ai", "resume"], args].concat());
        command.current_dir(cwd).env("PWD", cwd);
        // SAFETY: `setsid` is async-signal-safe and touches no memory of this process.
        unsafe {
            command.pre_exec(|| rustix::process::setsid().map(drop).map_err(Into::into));
        }
        Process::spawn(command).wait()
    }

    /// This machine's host id.
    pub fn host_id(&self) -> String {
        let status = self.run(&["store", "status"]);
        status
            .lines()
            .find_map(|l| l.strip_prefix("host: ")?.strip_suffix(" <- CURRENT HOST"))
            .unwrap_or_else(|| panic!("no current host in {status}"))
            .to_owned()
    }

    /// The way atuin names this machine to another: `@` and the end of its host id.
    pub fn short_host(&self) -> String {
        let id = self.host_id();
        format!("@{}", &id[id.len() - 8..])
    }

    /// Atuin's data directory here.
    pub fn data_dir(&self) -> PathBuf {
        self.env.data_dir()
    }
}

impl Drop for Machine {
    fn drop(&mut self) {
        if self.daemon.is_some() {
            let _ = Process::spawn(self.env.atuin(&["daemon", "stop"])).try_wait();
        }
    }
}

/// The text of each message of `show` (`atuin ai session show --style json`) that says something
/// in conversation: a prompt, or a reply.
pub fn said_in(show: &Value) -> Vec<String> {
    show["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["role"] == "user" || m["role"] == "assistant")
        .flat_map(|m| m["content"].as_array().unwrap().iter())
        .filter(|c| c["type"] == "text")
        .map(|c| c["text"].as_str().unwrap().to_owned())
        .collect()
}

/// One tool call of a session, as `atuin ai session show --style json` gives it.
#[derive(Debug, PartialEq, Eq)]
pub struct Call {
    pub name: String,
    /// What it was given, as JSON text; empty when capture kept none.
    pub input: String,
    /// What it returned; empty when capture kept none.
    pub output: String,
}

/// Every tool call of `show`, in order, each with what it returned.
pub fn calls_in(show: &Value) -> Vec<Call> {
    let content: Vec<&Value> = show["messages"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|m| m["content"].as_array().unwrap())
        .collect();
    let text = |value: &Value| value.as_str().unwrap_or_default().to_owned();
    content
        .iter()
        .filter(|c| c["type"] == "tool_call")
        .map(|call| Call {
            name: text(&call["name"]),
            input: text(&call["input"]),
            output: content
                .iter()
                .find(|c| c["type"] == "tool_result" && c["tool_use_id"] == call["id"])
                .map(|result| text(&result["content"]))
                .unwrap_or_default(),
        })
        .collect()
}

/// Whether `haystack` holds each of `needles`, in order.
pub fn holds_in_order(haystack: &str, needles: &[String]) -> bool {
    let mut rest = haystack;
    needles.iter().all(|needle| match rest.find(needle.as_str()) {
        Some(at) => {
            rest = &rest[at + needle.len()..];
            true
        }
        None => false,
    })
}

/// `output`'s stdout and stderr, asserting it succeeded.
pub fn succeeded(output: &Output) -> (String, String) {
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(output.status.success(), "failed:\nstdout: {stdout}\nstderr: {stderr}");
    (stdout, stderr)
}

/// `output`'s stderr, asserting it failed.
pub fn failed(output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        !output.status.success(),
        "succeeded:\nstdout: {}\nstderr: {stderr}",
        String::from_utf8_lossy(&output.stdout)
    );
    stderr
}
