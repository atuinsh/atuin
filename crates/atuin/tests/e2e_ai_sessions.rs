//! AI coding-agent sessions, end to end: captured from each agent's own transcripts by the daemon,
//! synced between machines, and resumed with `atuin ai resume` -- restored, caught up, forked,
//! switched or continued in another agent -- by the real binary, against transcripts written the
//! way each agent writes them.
//!
//! Each test checks something a user relies on that no single crate can see whole: that what an
//! agent wrote is what atuin shows and syncs, minus what it must never keep; that a session
//! written back out reads as the same session to the agent and to atuin; and that what `resume`
//! runs is the agent, on that session, where it ran.

#![cfg(all(unix, feature = "daemon", feature = "ai", feature = "sync"))]

mod common;

#[allow(dead_code, reason = "shared with the PTY tests")]
#[path = "common/agents.rs"]
mod agents;
#[allow(dead_code, reason = "shared with the PTY tests")]
#[path = "common/ai.rs"]
mod ai;

use std::path::Path;
use std::process::Stdio;

use agents::{Agent, Transcript, Turn, said, whole, word};
use ai::{Machine, SyncServer, shared_dir, succeeded};
use atuin_common::harnesstools::resume::quote;
use atuin_common::path::PathBufExt as _;
use atuin_common::secrets::REDACTED;
use regex::Regex;
use rstest::{fixture, rstest};
use tempfile::TempDir;

/// A machine on its own, its daemon capturing.
#[fixture]
fn machine() -> Machine {
    let mut machine = Machine::new();
    machine.start_daemon();
    machine
}

/// Where the sessions under test ran.
#[fixture]
fn project() -> TempDir {
    shared_dir()
}

/// Whether `query` finds session `id` in `machine`'s search.
fn finds(machine: &Machine, query: &str, id: &str) -> bool {
    let found = machine.run(&["ai", "session", "search", query, "--style", "json"]);
    let found: serde_json::Value = serde_json::from_str(&found).unwrap();
    found.as_array().unwrap().iter().any(|m| m["session"]["session_id"] == id)
}

/// The last word of `text`: the one [`agents::word`] made, found nowhere else.
fn last_word(text: &str) -> &str {
    text.split(' ').next_back().unwrap()
}

/// An agent's transcript is captured whole: listed with where it ran, its conversation in order,
/// and each tool call with what it was given and what it returned -- or, with `capture_tools`
/// off, the tool's name alone. Reasoning is kept only as a marker, a secret anywhere is redacted,
/// and search finds whole words of what was said and what a call was given, never what it
/// returned.
#[rstest]
fn capture_keeps_the_conversation_and_tool_calls(
    project: TempDir,
    #[values(Agent::Claude, Agent::Codex, Agent::Opencode, Agent::Pi)] agent: Agent,
    #[values(true, false)] capture_tools: bool,
) {
    let ai = format!("capture_sessions = true\ncapture_tools = {capture_tools}\n");
    let mut machine = Machine::with_config("", &ai);
    machine.start_daemon();
    let secret = format!("ghp_{}{}", &word()[1..], "abcd");
    let mut turns = vec![Turn::new(), Turn::new()];
    turns[1].prompt = format!("use the token {secret} for {}", word());
    turns[1].output = format!("token {secret} for {}", word());
    let transcript = Transcript::create(machine.home(), agent, project.path(), &turns);

    let mut want = said(&turns);
    want[2] = want[2].replace(&secret, REDACTED);
    machine.wait_until_said(&transcript.id, &want);

    let session = machine.session(&transcript.id).unwrap();
    assert_eq!(session["harness"], agent.harness());
    assert_eq!(session["cwd"], project.path().to_str().unwrap());

    let show = machine.show(&transcript.id);
    let calls = ai::calls_in(&show);
    assert_eq!(calls.len(), turns.len(), "one call a turn: {calls:#?}");
    for (call, turn) in calls.iter().zip(&turns) {
        assert_eq!(call.name, agent.tool());
        let output = turn.output.replace(&secret, REDACTED);
        if capture_tools {
            assert!(call.input.contains(&turn.command), "{call:#?}");
            assert!(call.output.contains(&output), "{call:#?}");
        } else {
            assert_eq!((call.input.as_str(), call.output.as_str()), ("", ""), "{call:#?}");
        }
    }
    let shown = show.to_string();
    for hidden in [&turns[0].thinking, &turns[1].thinking, &secret] {
        assert!(!shown.contains(hidden.as_str()), "{hidden:?} was captured: {shown}");
    }

    let id = &transcript.id;
    for said in [&turns[0].prompt, &turns[1].reply] {
        assert!(finds(&machine, last_word(said), id), "{said:?} didn't find the session");
    }
    // Words match whole; only the picker, searching as you type, matches a prefix.
    let prefix = &last_word(&turns[0].prompt)[..20];
    assert!(!finds(&machine, prefix, id), "{prefix:?} found the session");
    let command = last_word(&turns[0].command);
    assert_eq!(finds(&machine, command, id), capture_tools, "search for the command");
    for hidden in [&turns[0].thinking, &turns[0].output, &secret] {
        assert!(!finds(&machine, last_word(hidden), id), "{hidden:?} found the session");
    }
}

/// The history and output filters apply to the commands an agent runs as to the ones typed: one
/// history would leave out is kept by its tool's name alone, and one whose output the output
/// capture would leave out keeps what it ran, not what it printed.
#[rstest]
fn capture_applies_the_history_and_output_filters_to_agent_commands(project: TempDir) {
    let top =
        "history_filter = [\"^echo dropped-\"]\n[output]\ncommand_filter = [\"^echo quiet-\"]\n";
    let mut machine = Machine::with_config(top, "capture_sessions = true\n");
    machine.start_daemon();
    let mut turns = vec![Turn::new(), Turn::new(), Turn::new()];
    turns[0].command = format!("echo dropped-{}", word());
    turns[1].command = format!("echo quiet-{}", word());
    let transcript = Transcript::create(machine.home(), Agent::Claude, project.path(), &turns);
    machine.wait_until_said(&transcript.id, &said(&turns));

    let calls = ai::calls_in(&machine.show(&transcript.id));
    let kept: Vec<(bool, bool)> = calls
        .iter()
        .zip(&turns)
        .map(|(call, turn)| {
            (call.input.contains(&turn.command), call.output.contains(&turn.output))
        })
        .collect();
    assert_eq!(kept, [(false, false), (true, false), (true, true)], "{calls:#?}");
    assert!(calls.iter().all(|call| call.name == Agent::Claude.tool()), "{calls:#?}");
}

/// A transcript the agent goes on writing is followed, and one it wrote to while the daemon was
/// down is picked up where it was left: either way, every turn is in the session exactly once.
#[rstest]
fn capture_follows_a_transcript_exactly_once_across_restarts(
    mut machine: Machine,
    project: TempDir,
    #[values(Agent::Claude, Agent::Codex, Agent::Opencode, Agent::Pi)] agent: Agent,
) {
    let turns: Vec<Turn> = (0..3).map(|_| Turn::new()).collect();

    let transcript = Transcript::create(machine.home(), agent, project.path(), &turns[..1]);
    machine.wait_until_said(&transcript.id, &said(&turns[..1]));

    transcript.append(project.path(), &turns[1..2]);
    machine.wait_until_said(&transcript.id, &said(&turns[..2]));

    machine.stop_daemon();
    transcript.append(project.path(), &turns[2..]);
    machine.start_daemon();
    machine.wait_until_said(&transcript.id, &said(&turns));
}

/// An opencode session renamed after its conversation is listed, and found, under its new title
/// at once, not only once another message carries it.
#[rstest]
fn a_renamed_session_is_listed_under_its_new_title(machine: Machine, project: TempDir) {
    let turns = [Turn::new()];
    let transcript = Transcript::create(machine.home(), Agent::Opencode, project.path(), &turns);
    machine.wait_until_said(&transcript.id, &said(&turns));

    let title = format!("renamed {}", word());
    transcript.rename(&title);
    ai::wait_for("the session to be listed under its new title", || {
        let listed = machine.session(&transcript.id).unwrap()["title"].clone();
        if listed == title.as_str() {
            Ok(())
        } else {
            Err(format!("it is listed as {listed}"))
        }
    });
    assert!(finds(&machine, last_word(&title), &transcript.id), "the new title didn't find it");
}

/// Turning capture on in the config takes effect in the running daemon, which captures the
/// sessions the agents work in from then on. One they wrote while it was off is history, left
/// alone until the agent goes on with it, and then captured whole; or until `atuin ai session
/// import` brings it in.
#[rstest]
fn capture_starts_when_the_config_turns_it_on(project: TempDir) {
    let mut machine = Machine::with_config("", "capture_sessions = false\n");
    machine.start_daemon();
    let old: Vec<Turn> = (0..2).map(|_| Turn::new()).collect();
    let resumed = Transcript::create(machine.home(), Agent::Claude, project.path(), &old[..1]);
    let idle = [Turn::new()];
    let untouched = Transcript::create(machine.home(), Agent::Claude, project.path(), &idle);
    // Longer than the daemon takes to see a new transcript when capturing.
    std::thread::sleep(std::time::Duration::from_secs(3));
    assert_eq!(machine.sessions(), Vec::<serde_json::Value>::new(), "captured while off");

    let config = machine.env.config_file();
    let on = std::fs::read_to_string(&config)
        .unwrap()
        .replace("capture_sessions = false", "capture_sessions = true");
    std::fs::write(&config, on).unwrap();
    // Longer than the daemon takes to see the config change.
    std::thread::sleep(std::time::Duration::from_secs(3));
    let new = [Turn::new()];
    let started = Transcript::create(machine.home(), Agent::Claude, project.path(), &new);
    machine.wait_until_said(&started.id, &said(&new));

    resumed.append(project.path(), &old[1..]);
    machine.wait_until_said(&resumed.id, &said(&old));
    assert_eq!(machine.session(&untouched.id), None, "history captured unasked");

    machine.run(&["ai", "session", "import"]);
    machine.wait_until_said(&untouched.id, &said(&idle));
}

/// The command `atuin ai resume --print` prints to resume `transcript` in its own agent, from
/// `cwd`.
fn resume_line(transcript: &Transcript, cwd: &Path) -> String {
    let id = transcript.id.as_str();
    let args = match transcript.agent {
        Agent::Claude => vec!["--resume", id],
        Agent::Codex => vec!["resume", id],
        Agent::Opencode => vec!["--session", id],
        Agent::Pi => vec!["--session", transcript.path.to_str().unwrap()],
    };
    let mut line =
        format!("cd -- {} && {}", quote(cwd.to_str().unwrap()), transcript.agent.program());
    for arg in args {
        line.push(' ');
        line.push_str(&quote(arg));
    }
    line
}

/// A subagent isn't a session to resume, but the session it works for, which is listed as its
/// parent: naming the subagent resumes that, saying so.
#[rstest]
fn a_subagent_resumes_as_the_session_it_works_for(machine: Machine, project: TempDir) {
    let turns = [Turn::new()];
    let parent = Transcript::create(machine.home(), Agent::Claude, project.path(), &turns);
    let subagent = Transcript::claude_subagent(&parent, project.path(), &[Turn::new()]);
    machine.wait_until_said(&parent.id, &said(&turns));
    let session = machine.wait_for_child(Agent::Claude, &parent.id);
    assert_eq!(session["session_id"], subagent.id.as_str());
    assert_eq!(session["parent_kind"], "subagent");

    let (stdout, stderr) = succeeded(&machine.resume(&[&subagent.id, "--print"], machine.home()));
    assert_eq!(stdout.trim(), resume_line(&parent, project.path()));
    assert!(stderr.contains(&format!("{} is a subagent", subagent.id)), "{stderr}");
}

/// Without a terminal (an agent, a script) the picker can't open: it says so, and how to resume a
/// session without it.
#[rstest]
#[case::no_query(&[])]
#[case::a_query(&["picker"])]
fn the_picker_needs_a_terminal(machine: Machine, #[case] args: &[&str]) {
    let stderr = ai::failed(&machine.resume(args, machine.home()));
    assert_eq!(stderr.lines().next().unwrap(), NEEDS_A_TERMINAL);
}

const NEEDS_A_TERMINAL: &str = "Error: the picker needs a terminal; to resume without one, name \
                                the session: `atuin ai resume <id>` (`atuin ai session list` \
                                lists them)";

/// Nor with only stdout on a terminal, and that not its own (`setsid -w atuin ai resume
/// </dev/null` in a terminal): there's no terminal to read keys from.
#[rstest]
fn the_picker_needs_a_terminal_to_read_keys_from(machine: Machine) {
    use std::os::unix::process::CommandExt;

    use rustix::fs::{Mode, OFlags};
    use rustix::pty::{OpenptFlags, grantpt, openpt, ptsname, unlockpt};

    let pty = openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY).unwrap();
    grantpt(&pty).unwrap();
    unlockpt(&pty).unwrap();
    let name = ptsname(&pty, Vec::new()).unwrap();
    let terminal = rustix::fs::open(name.as_c_str(), OFlags::RDWR | OFlags::NOCTTY, Mode::empty());
    let mut command = machine.atuin(&["ai", "resume"]);
    command.current_dir(machine.home()).stdin(Stdio::null()).stdout(terminal.unwrap());
    // SAFETY: `setsid` is async-signal-safe and touches no memory of this process.
    unsafe {
        command.pre_exec(|| rustix::process::setsid().map(drop).map_err(Into::into));
    }
    let stderr = ai::failed(&command.output().unwrap());
    assert_eq!(stderr.lines().next().unwrap(), NEEDS_A_TERMINAL);
}

/// A session whose transcript is on this machine resumes in its agent where it ran, named by the
/// agent's id, by its atuin id, by enough of the start or end of that, and is left as it was. How a name is
/// resolved is the same for every agent; where its transcript is, and how it resumes, isn't.
#[rstest]
#[case::claude(Agent::Claude, "agent id")]
#[case::codex(Agent::Codex, "agent id")]
#[case::opencode(Agent::Opencode, "agent id")]
#[case::pi(Agent::Pi, "agent id")]
#[case::by_atuin_id(Agent::Opencode, "atuin id")]
#[case::by_atuin_id_prefix(Agent::Opencode, "atuin id prefix")]
#[case::by_short_id(Agent::Opencode, "short id")]
fn a_local_session_resumes_in_its_agent_where_it_ran(
    machine: Machine,
    project: TempDir,
    #[case] agent: Agent,
    #[case] named_by: &str,
) {
    let turns = [Turn::new()];
    let transcript = Transcript::create(machine.home(), agent, project.path(), &turns);
    machine.wait_until_said(&transcript.id, &said(&turns));
    let before = std::fs::read(&transcript.path).unwrap();

    let atuin_id =
        machine.session(&transcript.id).unwrap()["atuin_id"].as_str().unwrap().to_owned();
    let name = match named_by {
        "agent id" => transcript.id.clone(),
        "atuin id" => atuin_id,
        // Its end, as the UI shows it.
        "short id" => atuin_id[atuin_id.len() - 8..].to_owned(),
        // Past the timestamp the id starts with, which sessions started together share.
        _ => atuin_id[..16].to_owned(),
    };
    let (stdout, stderr) = succeeded(&machine.resume(&[&name, "--print"], machine.home()));
    assert_eq!(stdout.trim(), resume_line(&transcript, project.path()), "{stderr}");
    assert_eq!(std::fs::read(&transcript.path).unwrap(), before, "the transcript changed");
}

/// Two machines of one account, and a session captured on the first, on the second written back
/// out from sync by resuming it there: what the second's agent would open.
struct Restored {
    _server: SyncServer,
    project: TempDir,
    here: Machine,
    there: Machine,
    turns: Vec<Turn>,
    /// The session on the machine it was captured on.
    original: Transcript,
    /// Its transcript written back out on the other.
    restored: Transcript,
}

fn restored(agent: Agent) -> Restored {
    let server = SyncServer::start();
    let project = project();
    let (there, here) = Machine::pair(&server);
    let turns = vec![Turn::new(), Turn::new()];
    let original = Transcript::create(there.home(), agent, project.path(), &turns);
    there.wait_until_said(&original.id, &said(&turns));
    here.wait_until_said(&original.id, &said(&turns));
    assert!(Transcript::find(here.home(), agent, &original.id).is_none());

    let (stdout, stderr) = succeeded(&here.resume(&[&original.id, "--print"], here.home()));
    let restored = Transcript::find(here.home(), agent, &original.id)
        .unwrap_or_else(|| panic!("nothing restored: {stdout} {stderr}"));
    assert_eq!(stdout.trim(), resume_line(&restored, project.path()), "{stderr}");
    // Where it ran is here too, and the status says what was written all the same.
    assert!(stderr.contains("atuin: restored the session from sync\n"), "{stderr}");
    assert!(!stderr.lines().any(|line| line.contains("locked")), "{stderr}");
    Restored {
        _server: server,
        project,
        here,
        there,
        turns,
        original,
        restored,
    }
}

/// A session captured on one machine is listed on another, under the same atuin id, and resuming
/// it there writes it back out as its agent's own transcript: the conversation and its tool calls
/// as they were, without the reasoning sync never held. The agent going on with it
/// there carries on the same session -- nothing in it read twice -- which syncs back.
#[rstest]
fn a_session_from_another_machine_restores_as_the_same_session(
    #[values(Agent::Claude, Agent::Codex, Agent::Pi)] agent: Agent,
) {
    let Restored {
        _server,
        project,
        here,
        there,
        turns,
        original,
        restored,
    } = restored(agent);
    assert_eq!(
        here.session(&original.id).unwrap()["atuin_id"],
        there.session(&original.id).unwrap()["atuin_id"]
    );

    let text = restored.read();
    assert!(ai::holds_in_order(&text, &whole(&turns)), "the session isn't all there:\n{text}");
    assert!(!text.contains("[ran a shell command]"), "a tool call became a note:\n{text}");
    for turn in &turns {
        assert!(!text.contains(&turn.thinking), "the reasoning was restored:\n{text}");
    }
    let said = said(&turns);

    let more = vec![Turn::new()];
    restored.append(project.path(), &more);
    let all = [said, agents::said(&more)].concat();
    here.wait_until_said(&original.id, &all);
    there.wait_until_said(&original.id, &all);
}

/// A session `atuin sync` brings down is listed as soon as it returns, though the daemon running
/// beside it never syncs itself.
#[rstest]
fn a_session_synced_by_hand_is_listed_at_once(project: TempDir) {
    let server = SyncServer::start();
    let (there, here) = Machine::pair_syncing(&server, 0, "capture_sessions = true\n");
    let turns = [Turn::new()];
    let original = Transcript::create(there.home(), Agent::Claude, project.path(), &turns);
    there.wait_until_said(&original.id, &said(&turns));

    let downloaded = Regex::new(r"\d+/(\d+) up/down").unwrap();
    ai::wait_for("a sync to download the session", || {
        let synced = here.run(&["sync"]);
        match downloaded.captures(&synced) {
            Some(counts) if &counts[1] != "0" => Ok(()),
            _ => Err(synced),
        }
    });
    assert!(here.session(&original.id).is_some(), "not listed once synced");
}

/// A copy behind the session's other machine catches up with it as it resumes, saying so, and
/// once caught up resumes as it is.
#[rstest]
fn a_copy_behind_another_machine_catches_up_as_it_resumes(
    #[values(Agent::Claude, Agent::Codex, Agent::Pi)] agent: Agent,
) {
    let Restored {
        _server,
        project,
        here,
        there,
        mut turns,
        original,
        restored,
    } = restored(agent);
    turns.push(Turn::new());
    original.append(project.path(), &turns[2..]);
    here.wait_until_said(&original.id, &said(&turns));

    let (stdout, stderr) = succeeded(&here.resume(&[&original.id, "--print"], here.home()));
    assert_eq!(stdout.trim(), resume_line(&restored, project.path()));
    let caught_up =
        Regex::new(&format!(r"^atuin: caught up: \d+ messages? from {}\n$", there.short_host()));
    assert!(caught_up.unwrap().is_match(&stderr), "{stderr}");
    let text = restored.read();
    assert!(ai::holds_in_order(&text, &said(&turns)), "not caught up:\n{text}");

    let (_, stderr) = succeeded(&here.resume(&[&original.id, "--print"], here.home()));
    assert_eq!(stderr, "", "caught up twice");
    assert_eq!(restored.read(), text);
}

/// A session restored on another machine and gone on with there catches up on the machine it
/// came from, though its last turn ran a tool capture kept without its input (a note in the
/// restored copy): the copy goes on from the turn's last row, as the original does.
#[rstest]
fn a_session_gone_on_with_elsewhere_catches_up_where_it_came_from(
    #[values(Agent::Claude, Agent::Pi)] agent: Agent,
    project: TempDir,
) {
    let server = SyncServer::start();
    let (there, here) =
        Machine::pair_syncing(&server, 1, "capture_sessions = true\ncapture_tools = false\n");
    let mut turns = vec![Turn::new()];
    let original = Transcript::create(there.home(), agent, project.path(), &turns);
    here.wait_until_said(&original.id, &said(&turns));
    succeeded(&here.resume(&[&original.id, "--print"], here.home()));
    let restored = Transcript::find(here.home(), agent, &original.id).unwrap();
    assert!(restored.read().contains("[ran a shell command]"));

    turns.push(Turn::new());
    restored.append(project.path(), &turns[1..]);
    there.wait_until_said(&original.id, &said(&turns));
    let (_, stderr) = succeeded(&there.resume(&[&original.id, "--print"], there.home()));
    let caught_up =
        Regex::new(&format!(r"^atuin: caught up: \d+ messages? from {}\n$", here.short_host()));
    assert!(caught_up.unwrap().is_match(&stderr), "{stderr}");
    assert!(ai::holds_in_order(&original.read(), &said(&turns)), "not caught up");
}

/// Where a copy and the session's other machine went different ways, resuming it is a choice:
/// it fails saying so, and naming the flags that choose. `--as-is` resumes the copy untouched,
/// `--fork` forks the newest branch into a new session, and `--switch` puts the copy on the
/// other machine's branch, keeping it whole as a backup.
#[rstest]
fn a_diverged_copy_resumes_only_as_chosen(
    #[values(Agent::Claude, Agent::Codex, Agent::Pi)] agent: Agent,
) {
    let Restored {
        _server,
        project,
        here,
        there,
        turns,
        original,
        restored,
    } = restored(agent);
    // Theirs is the newest, which a copy elsewhere would catch up to.
    let (ours, theirs) = (Turn::new(), Turn::new());
    restored.append(project.path(), std::slice::from_ref(&ours));
    original.append(project.path(), std::slice::from_ref(&theirs));
    ai::wait("both branches", || {
        let show = here.show(&original.id).to_string();
        show.contains(&theirs.reply) && show.contains(&ours.reply)
    });
    let copy = restored.read();
    let resume =
        |flags: &[&str]| here.resume(&[&[original.id.as_str()][..], flags].concat(), here.home());

    let stderr = ai::failed(&resume(&["--print"]));
    assert!(
        stderr.contains(&format!("this copy went another way than {}'s", there.short_host())),
        "{stderr}"
    );
    for flag in ["--as-is", "--fork", "--switch"] {
        assert!(stderr.contains(flag), "{flag} not offered: {stderr}");
    }
    assert_eq!(restored.read(), copy);

    let (stdout, _) = succeeded(&resume(&["--as-is", "--print"]));
    assert_eq!(stdout.trim(), resume_line(&restored, project.path()));
    assert_eq!(restored.read(), copy);

    let (_, stderr) = succeeded(&resume(&["--fork", "--print"]));
    assert!(stderr.contains(&format!("forked into a new {} session", agent.label())), "{stderr}");
    let fork = here.wait_for_child(agent, &original.id);
    assert_eq!(fork["parent_kind"], "fork");
    let fork_id = fork["session_id"].as_str().unwrap();
    let newest = [said(&turns), said(std::slice::from_ref(&theirs))].concat();
    here.wait_until_holds(fork_id, &newest);
    assert_eq!(restored.read(), copy);

    let (_, stderr) = succeeded(&resume(&["--switch", "--print"]));
    let backup = stderr
        .split("(your copy is at ")
        .nth(1)
        .and_then(|rest| rest.split(')').next())
        .unwrap_or_else(|| panic!("no backup named: {stderr}"));
    assert!(
        stderr.starts_with(&format!("atuin: switched to {}'s branch: ", there.short_host())),
        "{stderr}"
    );
    assert!(
        Path::new(backup)
            .starts_with(here.data_dir().joined("ai/switched").joined(agent.harness())),
        "{backup}"
    );
    assert_eq!(std::fs::read_to_string(backup).unwrap(), copy, "the backup isn't the copy");
    let text = restored.read();
    let theirs_now = [said(&turns), said(std::slice::from_ref(&theirs))].concat();
    assert!(ai::holds_in_order(&text, &theirs_now), "not switched:\n{text}");
    assert!(!text.contains(&ours.reply), "our branch is still in it:\n{text}");
}

/// Continuing a session that went different ways on two machines in another agent carries the
/// branch `--branch` names, never both interleaved; without one named, it continues nothing,
/// listing the branches to name.
#[rstest]
#[case::claude_to_codex(Agent::Claude, Agent::Codex)]
#[case::codex_to_claude(Agent::Codex, Agent::Claude)]
#[case::pi_to_codex(Agent::Pi, Agent::Codex)]
fn continuing_a_diverged_session_carries_one_branch(#[case] from: Agent, #[case] to: Agent) {
    let Restored {
        _server,
        project,
        here,
        there,
        turns,
        original,
        restored,
    } = restored(from);
    // Theirs is the newest.
    let (ours, theirs) = (Turn::new(), Turn::new());
    restored.append(project.path(), std::slice::from_ref(&ours));
    original.append(project.path(), std::slice::from_ref(&theirs));
    ai::wait("both branches", || {
        let show = here.show(&original.id).to_string();
        show.contains(&theirs.reply) && show.contains(&ours.reply)
    });

    let args = [original.id.as_str(), "--in", to.program(), "--print"];
    let stderr = ai::failed(&here.resume(&args, here.home()));
    assert!(
        stderr.contains("it went 2 ways: name the branch to continue with `--branch`:"),
        "{stderr}"
    );
    assert!(stderr.contains(&there.short_host()) && stderr.contains("this machine"), "{stderr}");

    let theirs_by = there.short_host();
    for (branch, kept, dropped) in [(&*theirs_by, &theirs, &ours), ("this", &ours, &theirs)] {
        let args = [&args[..], &["--branch", branch]].concat();
        let (stdout, stderr) = succeeded(&here.resume(&args, here.home()));
        let continued = Transcript::find(here.home(), to, last_word(stdout.trim()))
            .unwrap_or_else(|| panic!("nothing continued: {stdout} {stderr}"));
        let text = continued.read();
        let want = [said(&turns), said(std::slice::from_ref(kept))].concat();
        assert!(ai::holds_in_order(&text, &want), "{branch:?}: not carried over:\n{text}");
        assert!(!text.contains(&dropped.reply), "{branch:?}: the other branch came too:\n{text}");
    }
}

/// Forking writes the session out as a new one of its agent, with the whole conversation, linked
/// to it as a fork, and resumes that; the original is left as it was.
#[rstest]
fn forking_a_session_leaves_it_and_links_the_fork(
    machine: Machine,
    project: TempDir,
    #[values(Agent::Claude, Agent::Codex, Agent::Pi)] agent: Agent,
) {
    let turns = vec![Turn::new(), Turn::new()];
    let original = Transcript::create(machine.home(), agent, project.path(), &turns);
    machine.wait_until_said(&original.id, &said(&turns));
    let before = original.read();

    let (stdout, stderr) =
        succeeded(&machine.resume(&[&original.id, "--fork", "--print"], machine.home()));
    assert_eq!(stderr.trim(), format!("atuin: forked into a new {} session", agent.label()));
    let fork = machine.wait_for_child(agent, &original.id);
    assert_eq!(fork["parent_kind"], "fork");
    let fork =
        Transcript::find(machine.home(), agent, fork["session_id"].as_str().unwrap()).unwrap();
    assert_eq!(stdout.trim(), resume_line(&fork, project.path()));
    machine.wait_until_holds(&fork.id, &said(&turns));
    assert_eq!(original.read(), before, "the original changed");
}

/// Continuing a session in another agent writes it out as a new session of that agent, which
/// carries the conversation on, each shell command now that agent's own shell tool with what it
/// printed, and only the reasoning left out. With nothing turned into notes, the status line just
/// says where it continues. It is linked to the original as its continuation, and resumed.
#[rstest]
#[case::claude_to_codex(Agent::Claude, Agent::Codex)]
#[case::claude_to_pi(Agent::Claude, Agent::Pi)]
#[case::codex_to_claude(Agent::Codex, Agent::Claude)]
#[case::codex_to_pi(Agent::Codex, Agent::Pi)]
#[case::opencode_to_claude(Agent::Opencode, Agent::Claude)]
#[case::opencode_to_codex(Agent::Opencode, Agent::Codex)]
#[case::pi_to_claude(Agent::Pi, Agent::Claude)]
#[case::pi_to_codex(Agent::Pi, Agent::Codex)]
fn continuing_in_another_agent_carries_the_conversation(
    machine: Machine,
    project: TempDir,
    #[case] from: Agent,
    #[case] to: Agent,
) {
    let turns = vec![Turn::new(), Turn::new()];
    let original = Transcript::create(machine.home(), from, project.path(), &turns);
    machine.wait_until_said(&original.id, &said(&turns));

    let (stdout, stderr) = succeeded(
        &machine.resume(&[&original.id, "--in", to.program(), "--print"], machine.home()),
    );
    assert_eq!(stderr.trim(), format!("atuin: continuing in {}", to.label()));
    let continued = machine.wait_for_child(to, &original.id);
    assert_eq!(continued["parent_kind"], "continuation");
    assert_eq!(continued["parent"]["harness"], from.harness());
    let continued =
        Transcript::find(machine.home(), to, continued["session_id"].as_str().unwrap()).unwrap();
    assert_eq!(stdout.trim(), resume_line(&continued, project.path()));

    machine.wait_until_holds(&continued.id, &said(&turns));
    let calls = ai::calls_in(&machine.show(&continued.id));
    assert_eq!(calls.len(), turns.len(), "one call a turn: {calls:#?}");
    for (call, turn) in calls.iter().zip(&turns) {
        assert_eq!(call.name, to.tool(), "{call:#?}");
        assert!(call.input.contains(&turn.command), "{call:#?}");
        assert!(call.output.contains(&turn.output), "{call:#?}");
    }
    let text = continued.read();
    for turn in &turns {
        assert!(!text.contains(&turn.thinking), "the reasoning was carried over:\n{text}");
    }
}

/// The index is the record store's to rebuild: what every session says, and how sessions link,
/// is the same after `atuin store rebuild ai-session` as before.
#[rstest]
fn rebuilding_the_index_reproduces_it(machine: Machine, project: TempDir) {
    let mut ids = Vec::new();
    let turns = [Turn::new()];
    for agent in Agent::ALL {
        let transcript = Transcript::create(machine.home(), agent, project.path(), &turns);
        machine.wait_until_said(&transcript.id, &said(&turns));
        ids.push(transcript.id);
    }
    // A continuation, for the link between sessions.
    succeeded(&machine.resume(&[&ids[0], "--in", "codex", "--print"], machine.home()));
    let continued = machine.wait_for_child(Agent::Codex, &ids[0]);
    ids.push(continued["session_id"].as_str().unwrap().to_owned());
    machine.wait_until_holds(&ids[4], &said(&turns));

    let index = |machine: &Machine| -> Vec<serde_json::Value> {
        ids.iter().map(|id| machine.show(id)).collect()
    };
    let before = index(&machine);
    let out = machine.run(&["store", "rebuild", "ai-session"]);
    assert!(out.contains("rebuilding"), "{out}");
    // `ai session` commands wait for the rebuild.
    assert_eq!(index(&machine), before);
}

/// A message bigger than gRPC takes by default (4 MiB: a pasted log, a huge file) reads back
/// whole.
#[rstest]
fn a_huge_message_reads_back(machine: Machine, project: TempDir) {
    let mut turns = [Turn::new()];
    turns[0].prompt = format!("{} {}", "x".repeat(9 << 19), turns[0].prompt);
    let transcript = Transcript::create(machine.home(), Agent::Claude, project.path(), &turns);
    machine.wait_until_said(&transcript.id, &said(&turns));
}
