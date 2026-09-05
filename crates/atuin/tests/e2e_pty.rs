//! Interactive shell tests against a rendered PTY screen.
//! Set `ATUIN_E2E_REQUIRE_SHELLS=1` to fail on missing shells or ble.sh.

#![cfg(unix)]

mod common;

#[path = "common/pty.rs"]
mod pty;
#[path = "common/shell.rs"]
mod shell;

use std::path::PathBuf;

use common::{SESSION, marker, output, wait_until};
use pty::PtyShell;
use rstest::rstest;
use shell::{PROMPT, Shell};

/// Run a command in the shell and wait for its output line to render.
fn run_echo_marker(pty: &PtyShell, marker: &str) {
    pty.send_line(&format!("echo {marker}"));
    // Match output, not the echoed command line.
    pty.wait_for_line(marker);
}

/// Search for a unique command on a cleared screen.
fn search_for_marker(pty: &PtyShell, marker: &str, open_key: &[u8]) {
    pty.send_line("clear");
    pty.send(open_key);
    pty.wait_for(": exit");
    // The full command must appear as a result, not just in the query.
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
    // Fish and zsh finish history entries in the background; read afresh on each poll.
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

    // Appending text and executing proves the selection was left editable.
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
    pty.wait_for_screen("empty search dismissed", |s| {
        !s.contains(": exit") && s.lines().any(|l| l.trim() == PROMPT)
    });
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
    pty.wait_for_screen("selected command's exact output", |_| {
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
