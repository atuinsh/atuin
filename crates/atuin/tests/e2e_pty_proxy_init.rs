//! However the pty-proxy init is sourced, the shell it execs must keep its keyboard.
//!
//! Fish users are told to run `atuin pty-proxy init fish | source`, which puts the init script
//! itself on the sourcing shell's stdin. Fish keeps the redirections in effect across `exec`, so
//! an unqualified `exec atuin pty-proxy` hands the proxy that pipe as its stdin instead of the
//! terminal. The proxy reads EOF straight away and stops forwarding input, so the shell it hosts
//! never sees a keystroke -- nor a reply to the terminal queries it makes at startup (#4220).
//!
//! The preamble repairs that by falling back to `/dev/tty`, but only when stdin is not already a
//! terminal, so both sourcing styles are driven here: the pipe that needs the fallback, and the
//! file that must not take it.
//!
//! The preamble must also leave alone a Bash or Zsh started as `<shell> -ic <command>` or
//! `<shell> -i <file>`: the shell the proxy would start never gets the command or the file, so it
//! sits waiting on the terminal for input instead of running them.

#![cfg(all(unix, feature = "pty-proxy"))]

mod common;

#[allow(dead_code, reason = "the pty harness is shared with the other e2e test binaries")]
#[path = "common/pty.rs"]
mod pty;
#[allow(dead_code, reason = "only `PROMPT` is needed here; `pty` reaches for it by path")]
#[path = "common/shell.rs"]
mod shell;

use std::collections::BTreeMap;
use std::os::unix::fs::MetadataExt as _;
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use atuin_common::path::PathBufExt as _;
use common::FreshEnv;
use pty::PtyShell;
use rstest::rstest;
use shell::PROMPT;

/// How the pty-proxy preamble reaches the shell, and what that leaves on its stdin.
#[derive(Clone, Copy, Debug)]
enum Sourced {
    /// The setup from #4220: a pipe, so the shell that execs the proxy has no terminal on stdin.
    FromPipe,
    /// A plain file, so stdin is still the shell's own terminal and needs no repair.
    FromFile,
}

/// `config.fish` for `sourced`, minus the line that loads the preamble.
const CONFIG_FISH: &str = r#"atuin init fish | source
function fish_prompt
    echo -n "{prompt} "
end
function fish_mode_prompt
end
"#;

/// The `name` binary to test against, or [`None`] when it is not installed.
///
/// Mirrors the lookup in [`shell::Shell::start`]: `ATUIN_E2E_<NAME>` overrides the binary, and a
/// missing shell is a hard error only when `ATUIN_E2E_REQUIRE_SHELLS` is set.
fn find_shell(name: &str) -> Option<PathBuf> {
    if let Some(path) = atuin_common::env::var_os(format!("ATUIN_E2E_{}", name.to_uppercase())) {
        let path = PathBuf::from(path);
        assert!(
            path.is_file(),
            "ATUIN_E2E_{} is not a file: {}",
            name.to_uppercase(),
            path.display()
        );
        return Some(path);
    }
    let found = std::env::split_paths(&atuin_common::env::var_os("PATH").unwrap_or_default())
        .map(|dir| dir.joined(name))
        .find(|path| path.is_file());
    if found.is_none() {
        assert!(
            atuin_common::env::var_os("ATUIN_E2E_REQUIRE_SHELLS").is_none(),
            "missing {name} for e2e_pty_proxy_init"
        );
        eprintln!("skipping e2e_pty_proxy_init: missing {name}");
    }
    found
}

#[rstest]
fn fish_keeps_its_keyboard_however_the_preamble_is_sourced(
    #[values(Sourced::FromPipe, Sourced::FromFile)] sourced: Sourced,
) {
    let Some(fish) = find_shell("fish") else {
        return;
    };

    let env = FreshEnv::new();
    let load = match sourced {
        Sourced::FromPipe => "atuin pty-proxy init fish | source".to_string(),
        Sourced::FromFile => {
            let script = env.home().join("pty-proxy-init.fish");
            std::fs::write(&script, env.run(&["pty-proxy", "init", "fish"])).unwrap();
            format!("source {}", script.display())
        }
    };
    let config = env.home().join(".config/fish/config.fish");
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    std::fs::write(&config, format!("{load}\n{}", CONFIG_FISH.replace("{prompt}", PROMPT)))
        .unwrap();

    let pty = PtyShell::spawn(&fish, &[], &env, &BTreeMap::new());
    pty.wait_for_prompt();

    // Typing has to reach the shell the proxy hosts, and the proxy has to have taken over -- a
    // preamble that quietly declined to exec would keep the keyboard working too.
    pty.send_line("test -n \"$ATUIN_PTY_PROXY_ACTIVE\" && echo proxy=1 || echo proxy=0");
    pty.wait_for_line("proxy=1");
}

/// Whether a login bash -- one with a leading `-` in argv[0] -- would report `bash` as `$BASH`.
///
/// Such a bash takes `$BASH` from the user's passwd entry rather than from argv[0], and the
/// preamble both checks against and spawns `$BASH`, so the login cases only mean something when
/// that's the bash under test.
fn login_bash_is(bash: &Path) -> bool {
    let output = Command::new(bash)
        .arg0("-bash")
        .args(["--noprofile", "--norc", "-c", r#"printf %s "$BASH""#])
        .output()
        .unwrap();
    let reported = std::fs::metadata(String::from_utf8_lossy(&output.stdout).as_ref());
    let bash = std::fs::metadata(bash).unwrap();
    reported.is_ok_and(|reported| (reported.dev(), reported.ino()) == (bash.dev(), bash.ino()))
}

/// Whether `bash` sets `$0` to the script name before reading its startup files, which it does
/// from 5.2 on. Before that, `bash -i <file>` looks just like `bash -i` to the preamble.
fn bash_names_script_in_rc(bash: &Path) -> bool {
    Command::new(bash)
        .args([
            "--norc",
            "-c",
            "(( BASH_VERSINFO[0] > 5 || (BASH_VERSINFO[0] == 5 && BASH_VERSINFO[1] >= 2) ))",
        ])
        .status()
        .unwrap()
        .success()
}

/// How a Bash or Zsh is started under the preamble.
#[derive(Clone, Copy, Debug)]
enum Invocation {
    /// `<shell> -i`: an ordinary interactive shell, which the proxy should take over.
    Interactive,
    /// A login shell, `-<shell>` in argv[0], started next to a file of that name: still an
    /// ordinary interactive shell.
    Login,
    /// A login shell from `exec -l <path>`, which puts `-<path>` in argv[0].
    LoginPath,
    /// The shell started by bare name from `PATH`, with an rc that defines a function of that
    /// name before the preamble, which must still find the shell binary rather than the function.
    ShadowedByFunction,
    /// The shell started through a relative path to a symlink, which `$BASH` resolves to an
    /// absolute path but argv[0] keeps as given: still an ordinary interactive shell.
    RelativePath,
    /// `<shell> -ic <command>`.
    Command,
    /// `<shell> -ic <command> <name>`, which sets `$0` to `<name>`.
    NamedCommand,
    /// `<shell> -i <file>`.
    File,
}

/// Prints whether the shell is running inside a proxy.
const REPORT_PROXY: &str = r#"echo "proxy=${ATUIN_PTY_PROXY_ACTIVE:+1}""#;

#[rstest]
fn posix_shells_run_their_command_or_file_instead_of_a_proxy(
    #[values("bash", "zsh")] name: &str,
    #[values(
        Invocation::Interactive,
        Invocation::Login,
        Invocation::LoginPath,
        Invocation::ShadowedByFunction,
        Invocation::RelativePath,
        Invocation::Command,
        Invocation::NamedCommand,
        Invocation::File
    )]
    invocation: Invocation,
) {
    let Some(shell) = find_shell(name) else {
        return;
    };

    let env = FreshEnv::new();
    if let Invocation::Login | Invocation::LoginPath = invocation
        && name == "bash"
        && !login_bash_is(&shell)
    {
        eprintln!("skipping: a login bash's $BASH is the passwd shell, not {}", shell.display());
        return;
    }
    if let Invocation::File = invocation
        && name == "bash"
        && !bash_names_script_in_rc(&shell)
    {
        eprintln!("skipping: {} only sets $0 to the script after its rc", shell.display());
        return;
    }
    let shadow = match invocation {
        Invocation::ShadowedByFunction => format!("{name}() {{ command {name} \"$@\"; }}\n"),
        _ => String::new(),
    };
    std::fs::write(
        env.home().join(format!(".{name}rc")),
        format!("{shadow}eval \"$(atuin pty-proxy init {name})\"\nPS1='{PROMPT} '\n"),
    )
    .unwrap();
    // A login bash reads `.bash_profile` rather than `.bashrc`, after an `/etc/profile` that may
    // have reset `PATH`.
    std::fs::write(
        env.home().join(".bash_profile"),
        format!("PATH='{}'\n. ~/.bashrc\n", env.path_var()),
    )
    .unwrap();
    let script = env.home().join("report-proxy.sh");
    std::fs::write(&script, format!("{REPORT_PROXY}\n")).unwrap();

    // A link to the shell under test, for starting it by a path or name of our choosing.
    let shells = env.home().join("shells");
    std::fs::create_dir_all(&shells).unwrap();
    std::os::unix::fs::symlink(&shell, shells.join(name)).unwrap();
    let mut vars = BTreeMap::new();

    let args: Vec<String> = match invocation {
        Invocation::Interactive => vec!["-i".into()],
        Invocation::ShadowedByFunction => {
            vars.insert("PATH".into(), format!("{}:{}", shells.display(), env.path_var()));
            vec!["-c".into(), format!("exec {name} -i")]
        }
        Invocation::Login => {
            std::fs::write(env.home().join(format!("-{name}")), "").unwrap();
            vec!["-c".into(), format!("exec -a -{name} '{}' -i", shell.display())]
        }
        Invocation::LoginPath => vec!["-c".into(), format!("exec -l '{}' -i", shell.display())],
        Invocation::RelativePath => vec!["-c".into(), format!("exec ./shells/{name} -i")],
        Invocation::Command => vec!["-ic".into(), REPORT_PROXY.into()],
        Invocation::NamedCommand => vec!["-ic".into(), REPORT_PROXY.into(), "report".into()],
        Invocation::File => vec!["-i".into(), script.display().to_string()],
    };
    let pty = PtyShell::spawn(&shell, &args, &env, &vars);

    if let Invocation::Interactive
    | Invocation::Login
    | Invocation::LoginPath
    | Invocation::ShadowedByFunction
    | Invocation::RelativePath = invocation
    {
        pty.wait_for_prompt();
        pty.send_line(REPORT_PROXY);
        pty.wait_for_line("proxy=1");
    } else {
        // A proxied shell would never print this: it would be waiting at a prompt instead.
        pty.wait_for_line("proxy=");
    }
}
