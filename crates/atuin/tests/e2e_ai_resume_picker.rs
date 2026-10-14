//! `atuin ai resume`'s picker on a terminal: choosing a session, and where to resume it, runs the
//! agent on it -- straight from the picker, and through each shell's widget, with and without
//! `enter_accept`.

#![cfg(all(unix, feature = "daemon", feature = "ai"))]

mod common;

#[allow(dead_code, reason = "shared with the other AI session tests")]
#[path = "common/agents.rs"]
mod agents;
#[allow(dead_code, reason = "shared with the other AI session tests")]
#[path = "common/ai.rs"]
mod ai;
#[allow(dead_code, reason = "shared with the other e2e test binaries")]
#[path = "common/pty.rs"]
mod pty;
#[allow(dead_code, reason = "shared with the other e2e test binaries")]
#[path = "common/shell.rs"]
mod shell;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use agents::{Agent, Transcript, Turn, ran, said};
use ai::{Machine, shared_dir};
use pty::PtyShell;
use rstest::rstest;

/// A machine with two Claude Code sessions captured, both run in `project`: the first is the one
/// to pick.
fn two_sessions(machine: &mut Machine, project: &Path) -> (Transcript, Turn) {
    machine.start_daemon();
    let (wanted, other) = (Turn::new(), Turn::new());
    let other_session =
        Transcript::create(machine.home(), Agent::Claude, project, std::slice::from_ref(&other));
    let session =
        Transcript::create(machine.home(), Agent::Claude, project, std::slice::from_ref(&wanted));
    for (session, turn) in [(&session, &wanted), (&other_session, &other)] {
        machine.wait_until_said(&session.id, &said(std::slice::from_ref(turn)));
    }
    (session, wanted)
}

/// Search the open picker for `turn`'s session by a word of its prompt, wait until it is the
/// one shown, and accept it: the chooser opens under it on where to resume it, its keys in the
/// header.
fn pick(pty: &PtyShell, turn: &Turn) {
    let word = turn.prompt.split(' ').next_back().unwrap();
    pty.wait_for("2 sessions");
    pty.send_str(word);
    pty.wait_for_screen("only the session searched for", |s| {
        s.contains(" 1 session") && s.contains(word)
    });
    pty.send_enter();
    pty.wait_for("esc back");
}

/// The digit of the chooser's line for continuing in `agent` (under its `Continue in` rule), once
/// it is drawn.
fn line_for(pty: &PtyShell, agent: Agent) -> char {
    let line = |s: &str| {
        s.lines()
            .skip_while(|l| !l.contains("Continue in"))
            .find(|l| l.contains(agent.label()))
            .map(str::to_owned)
    };
    let screen = pty.wait_for_screen(&format!("the line for {agent:?}"), |s| line(s).is_some());
    line(&screen).unwrap().chars().find(char::is_ascii_digit).unwrap()
}

/// With `enter_accept`, the picker runs the agent in place of itself: on the session chosen, in
/// its own agent, where it ran, or -- chosen by its digit in the chooser -- on a new session
/// continuing it in another.
#[rstest]
#[case::its_own_agent(None)]
#[case::another_agent(Some(Agent::Codex))]
fn the_picker_resumes_the_session_chosen(#[case] into: Option<Agent>) {
    let project = shared_dir();
    let mut machine = Machine::with_config("enter_accept = true\n", "capture_sessions = true\n");
    let (session, turn) = two_sessions(&mut machine, project.path());

    let vars = BTreeMap::from([("PATH".to_owned(), machine.path_var())]);
    let pty = PtyShell::spawn(
        Path::new(env!("CARGO_BIN_EXE_atuin")),
        &["ai".to_owned(), "resume".to_owned()],
        &machine.env,
        &vars,
    );
    pick(&pty, &turn);

    let Some(into) = into else {
        pty.send_enter();
        pty.wait_for(&ran(Agent::Claude, &format!("--resume {}", session.id), project.path()));
        return;
    };
    pty.send(line_for(&pty, into).to_string().as_bytes());
    let continued = machine.wait_for_child(into, &session.id);
    assert_eq!(continued["parent_kind"], "continuation");
    let id = continued["session_id"].as_str().unwrap();
    pty.wait_for(&ran(into, &format!("resume {id}"), project.path()));
}

/// Through the shell widget, the session chosen resumes in the shell: run at once with
/// `enter_accept`, else left on the command line to run.
#[rstest]
fn the_shell_widget_resumes_the_session_chosen(
    #[files("tests/shells/*.toml")] setup: PathBuf,
    #[values(false, true)] enter_accept: bool,
) {
    let Some(setup) = shell::Setup::find(&setup) else {
        return;
    };
    let project = shared_dir();
    let top = format!("enter_accept = {enter_accept}\n");
    let mut machine = Machine::with_config(&top, "capture_sessions = true\n");
    let (session, turn) = two_sessions(&mut machine, project.path());

    let vars = BTreeMap::from([("PATH".to_owned(), machine.path_var())]);
    let (pty, _) = setup.spawn(&machine.env, vars, "--bind-ai-resume");
    // ctrl-]
    pty.send(b"\x1d");
    pick(&pty, &turn);
    pty.send_enter();

    let command = format!("claude --resume {}", session.id);
    if !enter_accept {
        pty.wait_for_screen("the command on the command line", |s| {
            !s.contains("esc back")
                && s.lines().any(|l| l.contains(shell::PROMPT) && l.contains(&command))
        });
        assert!(!pty.screen().contains("AGENT-RAN"), "it ran before it was accepted");
        pty.send_enter();
    }
    pty.wait_for_line(&ran(Agent::Claude, &format!("--resume {}", session.id), project.path()));
}
