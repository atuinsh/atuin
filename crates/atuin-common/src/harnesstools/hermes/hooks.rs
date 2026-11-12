//! Atuin's plugin for Hermes Agent: `<home>/plugins/atuin/`, a `plugin.yaml` and an
//! `__init__.py` whose `pre_tool_call` and `post_tool_call` hooks hand each `terminal` call to
//! `atuin hook hermes`, and whose `hermes atuin import` command is how atuin writes a session
//! into the store. The plugin's own docs, and `rehydrate`, say more.
//!
//! A plugin rather than Hermes's shell hooks: those need the user's consent per command line,
//! recorded in a file, and their payload has no cwd. Hermes loads a plugin from the home's
//! `plugins/` only once `config.yaml` lists it in `plugins.enabled`, which `hermes plugins
//! enable` writes; that also loads it into a running gateway. Each profile is a home of its own,
//! with its own `plugins/` and `config.yaml`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::super::InstallHookError;
use super::cli::{HermesCli, HermesCommand};
use super::home::InstallHome;
use crate::path::PathBufExt as _;

const MANIFEST: &str = include_str!("../../../res/harnesstools/hermes/plugin.yaml");
const PLUGIN: &str = include_str!("../../../res/harnesstools/hermes/__init__.py");

/// The line of [`PLUGIN`] that install replaces with atuin's own path.
const ATUIN_LINE: &str = r#"ATUIN = "atuin""#;

/// The line of [`PLUGIN`] that install replaces with the variables atuin runs with.
const ENV_LINE: &str = "ATUIN_ENV = {}";

/// The plugin's name, which is also its directory's and what `hermes plugins enable` takes.
const NAME: &str = "atuin";

/// Write the plugin to `<home>/plugins/atuin/`, running `executable`, enable it, and return the
/// plugin's directory. [`InstallHookError::AlreadyInstalled`] when the files already say exactly
/// this.
///
/// `executable` is atuin by absolute path, since the gateway that runs hooks doesn't have the
/// shell's `PATH`: the one `mcp::launch_path` gives, which
/// outlasts a package manager's upgrade. Nor does it have the shell's `ATUIN_` settings, so the
/// plugin runs atuin with `env` ([`atuin_env`](super::atuin_env)) on top of the gateway's own.
pub async fn install(
    home: &InstallHome,
    executable: &Path,
    env: &BTreeMap<String, String>,
) -> Result<PathBuf, InstallHookError> {
    install_with(home.path(), executable, env, &HermesCommand::new(home)).await
}

/// [`install`], enabling the plugin with `hermes`.
///
/// The plugin is enabled even when it was already written, so a rerun after a failed enable
/// (Hermes not on `PATH` yet) finishes the job. A failed enable is not an error: the plugin is
/// in place, so the user is told the command to run.
async fn install_with(
    home: &Path,
    executable: &Path,
    env: &BTreeMap<String, String>,
    hermes: &impl HermesCli,
) -> Result<PathBuf, InstallHookError> {
    let dir = home.join("plugins").joined(NAME);
    tokio::fs::create_dir_all(&dir).await?;

    // The manifest last: Hermes finds a plugin by it.
    let files =
        [("__init__.py", plugin_source(executable, env)?), ("plugin.yaml", MANIFEST.to_owned())];
    let mut changed = false;
    for (name, contents) in files {
        let path = dir.join(name);
        match tokio::fs::read_to_string(&path).await {
            Ok(existing) if existing == contents => continue,
            Ok(_) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(err.into()),
        }
        tokio::fs::write(&path, contents).await?;
        changed = true;
    }

    let enable = ["plugins", "enable", NAME];
    if let Err(err) = hermes.run(&enable).await {
        crate::userio::log::warn!(
            "hermes-plugin-enable-failed",
            error = err.to_string(),
            command = hermes.command_line(&enable)
        );
    }

    if changed {
        Ok(dir)
    } else {
        Err(InstallHookError::AlreadyInstalled)
    }
}

/// [`PLUGIN`] with `executable` in place of the bare `atuin` and `env` in place of the empty
/// mapping, as Python literals: a JSON string is one, escapes and all, and so is a JSON object of
/// strings.
fn plugin_source(
    executable: &Path,
    env: &BTreeMap<String, String>,
) -> Result<String, InstallHookError> {
    let path = executable
        .to_str()
        .ok_or_else(|| InstallHookError::NonUtf8Executable(executable.to_owned()))?;
    let env = serde_json::to_string(env).expect("a map of strings is JSON");
    Ok(PLUGIN
        .replacen(ATUIN_LINE, &format!("ATUIN = {}", serde_json::Value::from(path)), 1)
        .replacen(ENV_LINE, &format!("ATUIN_ENV = {env}"), 1))
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::super::cli::fake::FakeHermes;
    use super::*;

    fn read(path: &Path) -> String {
        std::fs::read_to_string(path).unwrap()
    }

    #[rstest]
    fn the_plugin_names_atuin_once() {
        assert_eq!(PLUGIN.matches(ATUIN_LINE).count(), 1);
        assert_eq!(PLUGIN.matches(ENV_LINE).count(), 1);
    }

    #[rstest]
    #[tokio::test]
    async fn writes_the_plugin_and_enables_it() {
        let home = tempfile::tempdir().unwrap();
        let hermes = FakeHermes::default();

        let dir =
            install_with(home.path(), Path::new("/opt/atuin/bin/atuin"), &BTreeMap::new(), &hermes)
                .await
                .unwrap();

        assert_eq!(dir, home.path().join("plugins").joined("atuin"));
        assert_eq!(read(&dir.join("plugin.yaml")), MANIFEST);
        let source = read(&dir.join("__init__.py"));
        assert!(source.lines().any(|line| line == r#"ATUIN = "/opt/atuin/bin/atuin""#), "{source}");
        assert!(!source.contains(ATUIN_LINE));
        assert_eq!(hermes.ran(), ["plugins enable atuin"]);
    }

    /// The same files again change nothing, but the plugin is enabled again, in case it wasn't.
    #[rstest]
    #[tokio::test]
    async fn a_second_install_only_enables() {
        let home = tempfile::tempdir().unwrap();
        let hermes = FakeHermes::default();
        let atuin = Path::new("/opt/atuin/bin/atuin");
        let dir = install_with(home.path(), atuin, &BTreeMap::new(), &hermes).await.unwrap();
        let written = read(&dir.join("__init__.py"));

        assert!(matches!(
            install_with(home.path(), atuin, &BTreeMap::new(), &hermes).await,
            Err(InstallHookError::AlreadyInstalled)
        ));
        assert_eq!(read(&dir.join("__init__.py")), written);
        assert_eq!(hermes.ran(), ["plugins enable atuin", "plugins enable atuin"]);
    }

    #[rstest]
    #[tokio::test]
    async fn a_moved_binary_is_written_over() {
        let home = tempfile::tempdir().unwrap();
        let hermes = FakeHermes::default();
        install_with(home.path(), Path::new("/old/atuin"), &BTreeMap::new(), &hermes)
            .await
            .unwrap();

        let dir = install_with(home.path(), Path::new("/new/atuin"), &BTreeMap::new(), &hermes)
            .await
            .unwrap();

        let source = read(&dir.join("__init__.py"));
        assert!(source.lines().any(|line| line == r#"ATUIN = "/new/atuin""#), "{source}");
        assert!(!source.contains("/old/atuin"));
    }

    /// Without a `hermes` to enable it, the plugin is still written; the user is told what to
    /// run.
    #[rstest]
    #[tokio::test]
    async fn a_failed_enable_still_installs() {
        let home = tempfile::tempdir().unwrap();
        let hermes = FakeHermes::missing();

        let dir = install_with(home.path(), Path::new("/opt/atuin"), &BTreeMap::new(), &hermes)
            .await
            .unwrap();

        assert!(dir.join("__init__.py").is_file());
        assert!(dir.join("plugin.yaml").is_file());
        assert_eq!(hermes.ran(), ["plugins enable atuin"]);
    }

    #[rstest]
    fn the_path_is_a_python_string() {
        let source =
            plugin_source(Path::new(r#"/opt/Atuin's "bin"\atuin"#), &BTreeMap::new()).unwrap();
        assert!(source.contains(r#"ATUIN = "/opt/Atuin's \"bin\"\\atuin""#), "{source}");
    }

    #[rstest]
    fn the_env_is_a_python_dict() {
        let env =
            BTreeMap::from([("ATUIN_HOME".to_owned(), r#"/srv/Atuin's "home"\x"#.to_owned())]);
        let source = plugin_source(Path::new("/opt/atuin"), &env).unwrap();
        assert!(
            source.contains(r#"ATUIN_ENV = {"ATUIN_HOME":"/srv/Atuin's \"home\"\\x"}"#),
            "{source}"
        );
    }

    #[cfg(unix)]
    #[rstest]
    fn a_path_that_is_not_utf8_is_refused() {
        use std::os::unix::ffi::OsStrExt as _;
        let path = Path::new(std::ffi::OsStr::from_bytes(b"/opt/\xffatuin"));
        assert!(matches!(
            plugin_source(path, &BTreeMap::new()),
            Err(InstallHookError::NonUtf8Executable(_))
        ));
    }

    /// Whether `python3` is here to run the plugin.
    fn have_python() -> bool {
        std::process::Command::new("python3")
            .arg("--version")
            .output()
            .is_ok_and(|output| output.status.success())
    }

    /// A fake Hermes loading the installed plugin and calling its hooks as Hermes v0.23.0
    /// does, keyword arguments and all (`hermes-samples/plugin-hook.jsonl`). The Hermes
    /// internals the plugin asks for the command's directory return what the probes saw: no
    /// terminal directory before the first `cd`, the session's directory, and the terminal's
    /// after a `cd`.
    const DRIVER: &str = r#"
import importlib.util, json, sys, time, types

terminal_cwds = {}
def get_session_cwd(key):
    if key == "broken":
        raise RuntimeError("an internal API changed")
    return terminal_cwds.get(key)
session_key = ["agent:main:local:dm:s1"]

def fake_module(name, **attrs):
    module = types.ModuleType(name)
    module.__dict__.update(attrs)
    sys.modules[name] = module
fake_module("tools")
fake_module("agent")
fake_module("tools.approval", get_current_session_key=lambda default="default": session_key[0])
fake_module("tools.terminal_tool", get_session_cwd=get_session_cwd)
fake_module("agent.runtime_cwd", scoped_session_cwd=lambda: "/w")

spec = importlib.util.spec_from_file_location("atuin", sys.argv[1] + "/__init__.py")
plugin = importlib.util.module_from_spec(spec)
spec.loader.exec_module(plugin)
# macOS can take seconds to vet a new executable the first time it runs.
plugin._DRAIN_TIMEOUT_S = 60
hooks = {}
class Context:
    def register_hook(self, name, callback):
        hooks.setdefault(name, []).append(callback)
plugin.register(Context())
assert sorted(hooks) == ["post_tool_call", "pre_tool_call"], hooks

slowest = 0.0
def fire(name, **kwargs):
    global slowest
    for callback in hooks[name]:
        started = time.monotonic()
        assert callback(**kwargs) is None
        slowest = max(slowest, time.monotonic() - started)

def call(call_id, args, result, status="ok", tool="terminal", between=lambda: None):
    ids = {"session_id": "s1", "task_id": "s1", "tool_call_id": call_id, "turn_id": "t1",
           "conversation_id": "s1", "telemetry_schema_version": "hermes.observer.v1"}
    fire("pre_tool_call", tool_name=tool, args=args, middleware_trace=[], **ids)
    between()
    fire("post_tool_call", tool_name=tool, args=args, result=json.dumps(result), status=status,
         duration_ms=5, error_type=None, error_message=None, middleware_trace=[], **ids)

def cd(path):
    terminal_cwds["agent:main:local:dm:s1"] = path

call("c1", {"command": "true"}, {"output": "", "exit_code": 0, "error": None})
call("c2", {"command": "cd sub"}, {"output": "", "exit_code": 0, "error": None, "cwd": "/w/sub"},
     between=lambda: cd("/w/sub"))
call("c3", {"command": "ls /nope"}, {"output": "ls: /nope: No such file", "exit_code": 1, "error": None},
     status="error")
call("c4", {"command": "npm run dev", "background": True},
     {"output": "Background process started", "session_id": "proc_1", "pid": 42, "exit_code": 0, "error": None})
call("c5", {"command": "rm -rf /"}, {"output": "", "exit_code": -1, "error": "Command denied", "status": "blocked"},
     status="blocked")
call("c6", {"command": "make", "workdir": "build"}, {"output": "", "exit_code": 0, "error": None})
call("c7", {"command": "make", "workdir": "/elsewhere"}, {"output": "", "exit_code": 0, "error": None})
call("c8", {"path": "README.md"}, "hello", tool="read_file")
session_key[0] = "broken"
call("c9", {"command": "pwd"}, {"output": "/w", "exit_code": 0, "error": None})

# atuin sleeps through the first event: no callback may wait for it.
assert slowest < 0.5, f"a callback took {slowest:.2f}s"
"#;

    /// A fake Hermes loading the installed plugin, building `hermes atuin` from what the plugin
    /// registers as Hermes's `main.py` does, and running `hermes atuin import <file>` against a
    /// stand-in for the gateway's client, `hermes_cli.gateway_session_store`. The stand-in answers
    /// as the gateway did in the probes: a receipt counting what it imported, or a refusal.
    /// `argv[2]` names the case, and the driver prints the exit code after the command's own
    /// line.
    const IMPORT_DRIVER: &str = r#"
import argparse, importlib.util, json, sys, types

case = sys.argv[2]
calls = []

class GatewayClientError(Exception):
    def __init__(self, code, message):
        super().__init__(message)
        self.code = code

class Store:
    def call(self, verb, **kwargs):
        calls.append((verb, kwargs))
        if case == "refused":
            raise GatewayClientError("revision_conflict", "expected revision 0, found 3")
        if case == "skipped":
            return {"ok": True, "result": {"imported": 0, "skipped": 1}}
        return {"ok": True, "revision": 1, "result": {"imported": 1, "skipped": 0}}

if case != "old_hermes":
    for name in ("hermes_cli", "hermes_cli.gateway_session_store"):
        sys.modules[name] = types.ModuleType(name)
    sys.modules["hermes_cli.gateway_session_store"].session_store = Store

spec = importlib.util.spec_from_file_location("atuin", sys.argv[1] + "/__init__.py")
plugin = importlib.util.module_from_spec(spec)
spec.loader.exec_module(plugin)
commands = {}
class Context:
    def register_hook(self, name, callback):
        pass
    def register_cli_command(self, name, help, setup_fn, handler_fn):
        commands[name] = (setup_fn, handler_fn)
plugin.register(Context())
setup, handler = commands["atuin"]
parser = argparse.ArgumentParser(prog="hermes atuin")
setup(parser)

row = {"id": "20261110_120000_a1b2c3", "source": "cli", "messages": []}
with open(sys.argv[3], "w") as f:
    json.dump(row, f)
args = parser.parse_args(["import", sys.argv[3]] if case != "unreadable" else ["import", "/nope"])
code = handler(args)
if calls:
    verb, kwargs = calls[0]
    assert verb == "session.mutate", verb
    assert kwargs["session_id"] == row["id"], kwargs
    assert kwargs["operation"] == "import" and kwargs["expected_revision"] == 0, kwargs
    assert kwargs["payload"] == {"sessions": [row]}, kwargs
    assert len(kwargs["request_id"]) == 32, kwargs
print(code)
"#;

    /// `hermes atuin import` hands the gateway the row as a new session and prints one JSON line
    /// saying whether it was imported, exiting 0 only when it was; a Hermes without the
    /// gateway's client says so, rather than looking like a refusal.
    #[cfg(unix)]
    #[rstest]
    #[case::imported("imported", true, None, 0)]
    #[case::skipped("skipped", false, Some("skipped"), 1)]
    #[case::refused("refused", false, Some("revision_conflict: expected revision 0, found 3"), 1)]
    #[case::old_hermes("old_hermes", false, Some("unsupported_hermes"), 1)]
    #[case::unreadable("unreadable", false, Some("can't read /nope"), 1)]
    #[tokio::test]
    async fn the_plugin_imports_a_session_through_the_gateway(
        #[case] case: &str,
        #[case] ok: bool,
        #[case] error: Option<&str>,
        #[case] exit: i32,
    ) {
        if !have_python() {
            eprintln!("skipped: no python3");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let plugin =
            install_with(&home, Path::new("/opt/atuin"), &BTreeMap::new(), &FakeHermes::default())
                .await
                .unwrap();
        std::fs::write(dir.path().join("driver.py"), IMPORT_DRIVER).unwrap();

        let ran = std::process::Command::new("python3")
            .arg(dir.path().join("driver.py"))
            .arg(&plugin)
            .arg(case)
            .arg(dir.path().join("row.json"))
            .output()
            .unwrap();
        assert!(ran.status.success(), "{}", String::from_utf8_lossy(&ran.stderr));

        let stdout = String::from_utf8(ran.stdout).unwrap();
        let lines: Vec<&str> = stdout.lines().collect();
        let [reply, code] = lines.as_slice() else {
            panic!("not one reply and an exit code: {stdout}");
        };
        let reply: serde_json::Value = serde_json::from_str(reply).unwrap();
        assert_eq!(reply["ok"], ok, "{reply}");
        match error {
            Some(error) => {
                let said = reply["error"].as_str().unwrap();
                assert!(said.starts_with(error), "{said}");
            }
            None => assert!(reply.get("error").is_none(), "{reply}"),
        }
        assert_eq!(code.parse::<i32>().unwrap(), exit);
    }

    /// The plugin hands atuin each `terminal` call's start and end in order, with its
    /// directory, and nothing else; the callbacks return at once while atuin is busy, and
    /// what is still queued when Hermes exits is sent before it does.
    #[cfg(unix)]
    #[rstest]
    #[tokio::test]
    async fn the_plugin_sends_each_terminal_call_to_atuin() {
        use std::os::unix::fs::PermissionsExt;
        if !have_python() {
            eprintln!("skipped: no python3");
            return;
        }
        // next to the test binary: CI's temporary directory is mounted without exec. A quote
        // and a space in the path test the literal the path is written as.
        let exe = std::env::current_exe().unwrap();
        let dir = tempfile::Builder::new()
            .prefix("atuin's hermes ")
            .tempdir_in(exe.parent().unwrap())
            .unwrap();
        let log = dir.path().join("atuin.log");
        let stub = dir.path().join("atuin");
        let slept = dir.path().join("slept");
        let quote = |path: &Path| shlex::try_quote(path.to_str().unwrap()).unwrap().into_owned();
        std::fs::write(
            &stub,
            format!(
                "#!/bin/sh\n[ -e {slept} ] || {{ touch {slept}; sleep 1; }}\n{{ printf \
                 '%s\\t%s\\t' \"$*\" \"$ATUIN_HOME\"; cat; echo; }} >> {log}\n",
                slept = quote(&slept),
                log = quote(&log),
            ),
        )
        .unwrap();
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
        let home = dir.path().join("home");
        // What the installing shell set, which the gateway's environment doesn't have.
        let env = BTreeMap::from([("ATUIN_HOME".to_owned(), "/srv/atuin's home".to_owned())]);
        let plugin = install_with(&home, &stub, &env, &FakeHermes::default()).await.unwrap();
        std::fs::write(dir.path().join("driver.py"), DRIVER).unwrap();

        let ran = std::process::Command::new("python3")
            .arg(dir.path().join("driver.py"))
            .arg(&plugin)
            .output()
            .unwrap();
        assert!(ran.status.success(), "{}", String::from_utf8_lossy(&ran.stderr));

        let sent: Vec<serde_json::Value> = read(&log)
            .lines()
            .map(|line| {
                let mut fields = line.splitn(3, '\t');
                let (args, home, payload) =
                    (fields.next().unwrap(), fields.next().unwrap(), fields.next().unwrap());
                assert_eq!(args, "hook hermes");
                assert_eq!(home, "/srv/atuin's home");
                serde_json::from_str(payload).unwrap()
            })
            .collect();
        let seen: Vec<(&str, &str, &str)> = sent
            .iter()
            .map(|event| {
                let field = |name: &str| event[name].as_str().unwrap();
                (field("hook_event_name"), field("tool_call_id"), field("cwd"))
            })
            .collect();
        let both =
            |id, before, after| [("pre_tool_call", id, before), ("post_tool_call", id, after)];
        let want: Vec<(&str, &str, &str)> = [
            both("c1", "/w", "/w"),
            // the start is where the command began; the end, where the terminal now is
            both("c2", "/w", "/w/sub"),
            both("c3", "/w/sub", "/w/sub"),
            both("c4", "/w/sub", "/w/sub"),
            both("c5", "/w/sub", "/w/sub"),
            both("c6", "/w/sub/build", "/w/sub/build"),
            both("c7", "/elsewhere", "/elsewhere"),
            both("c9", "/w", "/w"),
        ]
        .concat();
        assert_eq!(seen, want);

        // Hermes's own arguments arrive as Hermes passed them.
        let end = &sent[5];
        assert_eq!(end["tool_name"], "terminal");
        assert_eq!(end["args"], serde_json::json!({"command": "ls /nope"}));
        assert_eq!(end["session_id"], "s1");
        assert_eq!(end["status"], "error");
        assert_eq!(end["duration_ms"], 5);
        let result: serde_json::Value =
            serde_json::from_str(end["result"].as_str().unwrap()).unwrap();
        assert_eq!(result["exit_code"], 1);
    }
}
