//! Interactive shell tests against a rendered PTY screen.

#![cfg(unix)]

mod common;

#[path = "common/pty.rs"]
mod pty;
#[path = "common/shell.rs"]
mod shell;

use std::path::PathBuf;

use common::{FreshEnv, SESSION, marker, output, wait_until};
use pty::PtyShell;
use rstest::rstest;
use shell::{PROMPT, Shell};

fn run_echo_marker(pty: &PtyShell, marker: &str) {
    pty.send_line(&format!("echo {marker}"));
    pty.wait_for_line(marker);
}

fn search_for_marker(pty: &PtyShell, marker: &str, open_key: &[u8]) {
    pty.send_line("clear");
    open_search_for(pty, marker, open_key);
}

/// Unlike `search_for_marker`, runs no command first: ble.sh drops keys typed right after the
/// first command.
fn open_search_for(pty: &PtyShell, marker: &str, open_key: &[u8]) {
    pty.send(open_key);
    pty.wait_for(": exit");
    // Search a suffix so the full command can only match a result.
    pty.send_str(&marker[marker.len() - 12..]);
    pty.wait_for(&format!("echo {marker}"));
}

fn executed_line(screen: &str, marker: &str) -> bool {
    screen.lines().any(|l| l.trim() == marker)
}

#[rstest]
fn shell_hooks_record_history(#[files("tests/shells/*.toml")] setup: PathBuf) {
    let Some(shell) = Shell::start(&setup, None) else {
        return;
    };
    let (env, pty) = (&shell.env, &shell.pty);

    let marker = marker();
    run_echo_marker(pty, &marker);

    let command = format!("sh -c 'exit 7' # {marker}");
    pty.send_line(&command);
    let cwd = env.home().canonicalize().unwrap();
    let expected = format!("7\t{}\t{command}", cwd.display());
    // Fish and zsh finish history entries asynchronously.
    wait_until("completed history entry with exit status and cwd", || {
        let mut command =
            env.atuin(&["history", "list", "--format", "{exit}\t{directory}\t{command}"]);
        command.env("ATUIN_SESSION", SESSION);
        output(command).lines().any(|l| l == expected)
    });
}

#[rstest]
fn selection_returns_for_editing(
    #[files("tests/shells/*.toml")] setup: PathBuf,
    #[values((true, b'\t'), (false, b'\t'), (false, b'\r'))] acceptance: (bool, u8),
) {
    let (enter_accept, key) = acceptance;
    let config = format!("enter_accept = {enter_accept}\n");
    let Some(shell) = Shell::start(&setup, Some(&config)) else {
        return;
    };
    let pty = &shell.pty;

    let marker = marker();
    run_echo_marker(pty, &marker);
    search_for_marker(pty, &marker, b"\x12");

    pty.send(&[key]);
    pty.wait_for_screen("selected command inserted at prompt", |s| {
        !s.contains(": exit")
            && !executed_line(s, &marker)
            && s.lines().any(|l| l.contains(PROMPT) && l.contains(&format!("echo {marker}")))
    });

    pty.send_str("-edited");
    pty.wait_for(&format!("echo {marker}-edited"));
    pty.send_enter();
    pty.wait_for_line(&format!("{marker}-edited"));
    assert!(!executed_line(&pty.screen(), &marker), "selection executed before editing");
}

#[rstest]
fn search_enter_accepts_and_runs(
    #[files("tests/shells/*.toml")] setup: PathBuf,
    #[values(b"\x12", b"\x1b[A")] open_key: &[u8],
) {
    let Some(shell) = Shell::start(&setup, Some("enter_accept = true\n")) else {
        return;
    };
    let pty = &shell.pty;

    let marker = marker();
    run_echo_marker(pty, &marker);
    search_for_marker(pty, &marker, open_key);

    pty.send_enter();
    pty.wait_for_screen("selected command executed", |s| {
        !s.contains(": exit") && executed_line(s, &marker)
    });
}

#[rstest]
fn empty_history_search_can_be_cancelled(#[files("tests/shells/*.toml")] setup: PathBuf) {
    let Some(shell) = Shell::start(&setup, None) else {
        return;
    };
    let pty = &shell.pty;
    pty.send_ctrl_r();
    pty.wait_for(": exit");
    pty.send(&[0x03]);
    // Wait for the line editor to actually resume (cursor back on the prompt, raw
    // mode) before typing. The search UI draws inline *below* the original prompt,
    // so a screen-only check for a prompt line can match that leftover prompt while
    // atuin is still tearing down and swallow the next command's keystrokes.
    pty.wait_for_prompt();
    assert!(!pty.screen().contains(": exit"), "search UI still visible after cancel");
    run_echo_marker(pty, &marker());
}

#[rstest]
fn selection_preserves_multiline_and_shell_quoting(
    #[files("tests/shells/*.toml")] setup: PathBuf,
    #[values(false, true)] enter_accept: bool,
) {
    let config = format!("enter_accept = {enter_accept}\n");
    let Some(shell) = Shell::start(&setup, Some(&config)) else {
        return;
    };
    let (env, pty) = (&shell.env, &shell.pty);
    let marker = marker();
    let expected = format!("{marker} café $HOME; \"quoted\"\nsecond line");
    let command = format!("printf '%s' '{expected}' > result.txt");
    env.record(&command, SESSION, env.home());

    pty.send_ctrl_r();
    pty.wait_for(": exit");
    pty.send_str(&marker[marker.len() - 12..]);
    pty.wait_for("printf");
    pty.send_enter();
    if !enter_accept {
        pty.wait_for_screen("multiline selection at prompt", |s| {
            !s.contains(": exit") && s.contains("printf") && s.contains("result.txt")
        });
        assert!(!env.home().join("result.txt").exists());
        pty.send(shell.config.multiline_accept.as_bytes());
    }
    wait_until("selected command's exact output", || {
        std::fs::read_to_string(env.home().join("result.txt")).is_ok_and(|s| s == expected)
    });
}

#[rstest]
fn search_survives_terminal_resize(
    #[files("tests/shells/*.toml")] setup: PathBuf,
    #[values((24, 80), (12, 60))] size: (u16, u16),
) {
    let Some(shell) = Shell::start(&setup, Some("enter_accept = true\n")) else {
        return;
    };
    let pty = &shell.pty;
    let marker = marker();
    run_echo_marker(pty, &marker);
    pty.send_line("clear");
    pty.send_ctrl_r();
    pty.wait_for(": exit");
    pty.resize(size.0, size.1);
    pty.send_str(&marker[marker.len() - 12..]);
    pty.wait_for(&format!("echo {marker}"));
    pty.send_enter();
    pty.wait_for_screen("selected command executed after resize", |s| {
        !s.contains(": exit") && executed_line(s, &marker)
    });
}

#[rstest]
fn filter_switching_changes_results(
    #[files("tests/shells/*.toml")] setup: PathBuf,
    #[values(false, true)] workspace: bool,
) {
    let Some(shell) = Shell::start(
        &setup,
        Some(
            "filter_mode = 'global'\nworkspaces = true\n[search]\nfilters = ['global', 'host', \
             'session', 'workspace', 'directory']\n",
        ),
    ) else {
        return;
    };
    let (env, pty) = (&shell.env, &shell.pty);
    let root = env.home().join("project");
    let cwd = root.join("current");
    let sibling = root.join("sibling");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&sibling).unwrap();
    if workspace {
        let mut git = std::process::Command::new("git");
        git.env_clear().envs(env.env_vars());
        git.args(["init", "--quiet"]).arg(&root);
        output(git);
    }
    pty.send_line("cd project/current");
    pty.send_line("echo $ATUIN_SESSION > session.txt");
    let session_file = cwd.join("session.txt");
    wait_until("shell session ID", || {
        std::fs::read_to_string(&session_file).is_ok_and(|s| !s.trim().is_empty())
    });
    let session = std::fs::read_to_string(session_file).unwrap();
    let marker = marker();
    let names = ["current", "directory", "workspace", "host", "global"];
    let commands = names.map(|name| format!("echo {marker}-{name}"));
    env.record(&commands[0], session.trim(), &cwd);
    env.record(&commands[1], SESSION, &cwd);
    env.record(&commands[2], SESSION, &sibling);
    env.record(&commands[3], SESSION, env.home());
    let mut remote = env.atuin(&["history", "start", "--", &commands[4]]);
    remote.env("ATUIN_SESSION", SESSION).env("ATUIN_HOST_NAME", "e2e-other-host");
    let id = output(remote);
    env.run(&["history", "end", "--exit", "0", id.trim()]);

    pty.send_line("clear");
    pty.send_ctrl_r();
    pty.wait_for(": exit");
    let query = &marker[marker.len() - 12..];
    pty.send_str(query);
    let mut modes = vec![("GLOBAL", 5), ("HOST", 4), ("SESSION", 1)];
    if workspace {
        modes.push(("WORKSPACE", 3));
    }
    modes.extend([("DIRECTORY", 2), ("GLOBAL", 5)]);
    for (index, (mode, count)) in modes.into_iter().enumerate() {
        if index > 0 {
            pty.send_ctrl_r();
        }
        pty.wait_for_screen(&format!("{mode} filter results"), |screen| {
            screen.lines().any(|line| line.contains(mode) && line.contains(query))
                && commands
                    .iter()
                    .enumerate()
                    .all(|(i, command)| screen.contains(command) == (i < count))
        });
    }
    pty.send(&[0x03]);
    pty.wait_for_screen("filter search closed", |s| !s.contains(": exit"));
    run_echo_marker(pty, &format!("{marker}-resumed"));
}

/// The `atuin ai resume` widget hands its pick back through a pipe, as the history widgets do,
/// and must not hand that pipe down: a process `atuin` leaves running (one an import started)
/// would hold it open, and the prompt would hang until it exited. Stands in for `atuin` with a
/// function that leaves one behind, with its standard error elsewhere.
#[rstest]
fn ai_resume_widget_returns_while_a_process_it_left_runs(
    #[files("tests/shells/*.toml")] setup: PathBuf,
) {
    let Some(shell) = Shell::start(&setup, None) else {
        return;
    };
    let pty = &shell.pty;
    let bind = match shell.config.shell.as_str() {
        "bash" => r"atuin-bind '\C-]' atuin-ai-resume",
        "zsh" => "bindkey '^]' atuin-ai-resume",
        // Fish's widget closes the descriptor itself (`3>&-`), and the stand-in below is a
        // POSIX function fish can't run.
        _ => return,
    };

    let marker = marker();
    // The process left behind is a program, as `atuin`'s are: `env sleep`, not `sleep`, which
    // ble.sh makes a shell function. A subshell left running that function would keep the
    // function's saved copies of the pipe (close-on-exec, but never exec'd), which `atuin`
    // itself, a program, never has.
    pty.send_line(&format!(
        "atuin() {{ if [ \"$1\" = ai ]; then (env sleep 60 2>/dev/null &); echo \
         \"__atuin_accept__:echo {marker}-resumed\" >&2; else command atuin \"$@\"; fi; }}"
    ));
    pty.send_line(bind);
    pty.send_line("clear");
    pty.send(&[0x1d]);
    // Within the wait's timeout, well before the process left behind exits.
    pty.wait_for_line(&format!("{marker}-resumed"));
}

const CTRL_O: &[u8] = b"\x0f";
const CTRL_T: &[u8] = b"\x14";

/// Binds `ctrl-t` in every mode a shell setup can start the TUI in, and in the inspector.
fn bind_ctrl_t(action: &str) -> String {
    ["emacs", "vim-insert", "inspector"]
        .map(|keymap| format!("[keymap.{keymap}]\n\"ctrl-t\" = \"{action}\"\n"))
        .concat()
}

/// `directory<TAB>command` for every history entry.
fn history_lines(env: &FreshEnv) -> String {
    let mut command = env.atuin(&["history", "list", "--format", "{directory}\t{command}"]);
    command.env("ATUIN_SESSION", SESSION);
    output(command)
}

#[rstest]
fn cd_action_changes_to_entry_directory(
    #[files("tests/shells/*.toml")] setup: PathBuf,
    #[values("accept-cd", "return-cd")] action: &str,
    #[values(false, true)] inspect: bool,
) {
    let Some(shell) = Shell::start(&setup, Some(&bind_ctrl_t(action))) else {
        return;
    };
    let (env, pty) = (&shell.env, &shell.pty);
    let marker = marker();
    // The trailing `\` quotes differently in POSIX shells and fish.
    let dir = env.home().join(r#"a b'c"d\e!$x\"#);
    std::fs::create_dir(&dir).unwrap();
    env.record(&format!("echo {marker}"), SESSION, &dir);

    open_search_for(pty, &marker, b"\x12");
    if inspect {
        pty.send(CTRL_O);
        pty.wait_for("[r] Runs");
    }
    pty.send(CTRL_T);
    if action == "return-cd" {
        pty.wait_for_screen("cd inserted at prompt", |s| {
            !s.contains(": exit") && s.lines().any(|l| l.contains(PROMPT) && l.contains("cd "))
        });
        // A line starting with `&&` is a syntax error, so this runs only if the `cd` is still
        // on the command line.
        pty.send_str(" && pwd > cwd.txt");
        pty.send_enter();
    } else {
        pty.wait_for_prompt();
        pty.send_line("pwd > cwd.txt");
    }
    // Relative path: the file lands in the entry's directory only if the `cd` ran.
    wait_until("shell moved to the entry's directory", || dir.join("cwd.txt").exists());
    wait_until("cd recorded in history", || {
        history_lines(env).lines().any(|l| l.contains("\tcd -- "))
    });
}

#[rstest]
fn cd_action_extends_command_chain(#[files("tests/shells/*.toml")] setup: PathBuf) {
    let config = format!("command_chaining = true\n{}", bind_ctrl_t("accept-cd"));
    let Some(shell) = Shell::start(&setup, Some(&config)) else {
        return;
    };
    let (env, pty) = (&shell.env, &shell.pty);
    let marker = marker();
    let dir = env.home().join("chained");
    std::fs::create_dir(&dir).unwrap();
    env.record(&format!("echo {marker}"), SESSION, &dir);

    pty.send_str("true &&");
    open_search_for(pty, &marker, b"\x12");
    pty.send(CTRL_T);
    // Chaining returns the extended line for editing instead of running it.
    pty.wait_for_screen("cd appended to the chain", |s| {
        !s.contains(": exit")
            && s.lines().any(|l| l.contains(PROMPT) && l.contains("true && cd -- "))
    });
    pty.send_str(" && pwd > cwd.txt");
    pty.send_enter();
    wait_until("chained cd ran", || dir.join("cwd.txt").exists());
}

#[rstest]
fn cd_action_without_directory_returns_original(#[files("tests/shells/*.toml")] setup: PathBuf) {
    // Imported entries are tagged `zsh`; `shells = "all"` keeps them visible from every shell.
    let config = format!("[search]\nshells = \"all\"\n{}", bind_ctrl_t("accept-cd"));
    let Some(shell) = Shell::start(&setup, Some(&config)) else {
        return;
    };
    let (env, pty) = (&shell.env, &shell.pty);
    let marker = marker();
    // Imported entries have no directory; atuin stores `unknown`.
    let histfile = env.home().join("imported_history");
    std::fs::write(&histfile, format!(": 1700000000:0;echo {marker}\n")).unwrap();
    let mut import = env.atuin(&["import", "zsh"]);
    import.env("HISTFILE", &histfile);
    output(import);

    open_search_for(pty, &marker, b"\x12");
    pty.send(CTRL_T);
    pty.wait_for_prompt();
    // Text left on the command line would prefix this command and break the exact match below.
    run_echo_marker(pty, &format!("{marker}-after"));
    wait_until("follow-up command recorded", || {
        history_lines(env).lines().any(|l| l.ends_with(&format!("\techo {marker}-after")))
    });
    let commands = history_lines(env);
    assert!(!commands.contains("\tcd "), "cd recorded:\n{commands}");
    // The imported entry is the only one without a directory; a run would record the shell's cwd.
    let ran = format!("\techo {marker}");
    assert!(
        !commands.lines().any(|l| l.ends_with(&ran) && !l.starts_with("unknown\t")),
        "entry ran:\n{commands}"
    );
}
