//! `atuin hook <agent>` stores what the agent reports a command printed.
//!
//! Claude Code cuts `stdout` at its own inline limit and points at the whole output on disk;
//! Codex sends the model-facing text as a bare string. Both must come back out of the daemon.

#![cfg(all(unix, feature = "daemon"))]

mod common;

use std::io::Write as _;
use std::process::Stdio;

use atuin_common::range::PyStyleIdxRange;
use atuin_daemon::client::HistoryClient;
use common::{FreshEnv, Process, SESSION, marker, output};

/// Feed one hook payload to `atuin hook <agent>`, as the agent would on stdin.
fn hook(env: &FreshEnv, agent: &str, payload: &serde_json::Value) {
    let mut cmd = env.atuin(&["hook", agent]);
    cmd.env("ATUIN_SESSION", SESSION).stdin(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().unwrap();
    child.stdin.take().unwrap().write_all(payload.to_string().as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "hook failed: {}", String::from_utf8_lossy(&out.stderr));
}

/// The id the daemon recorded `command` under.
fn id_of(env: &FreshEnv, command: &str) -> String {
    let mut list = env.atuin(&["history", "list", "--format", "{uuid}\t{command}"]);
    list.env("ATUIN_SESSION", SESSION);
    output(list)
        .lines()
        .filter_map(|line| line.split_once('\t'))
        .find(|(_, recorded)| *recorded == command)
        .map(|(id, _)| id.to_owned())
        .unwrap_or_else(|| panic!("{command} was never recorded"))
}

async fn stored(client: &mut HistoryClient, id: &str) -> String {
    let response = client
        .get_command_output(id.parse().unwrap(), vec![PyStyleIdxRange::new(0, -1)])
        .await
        .unwrap()
        .expect("the hook registered no output");
    response.chunks.iter().map(|chunk| chunk.content.as_str()).collect()
}

#[tokio::test]
async fn hooks_store_the_output_agents_report() {
    let env = FreshEnv::new();
    env.write_config(
        "[output]\nenabled = true\nmax_output_size = \"1MB\"\nsync = false\nmax_disk_usage = \
         \"unlimited\"\n[daemon]\nenabled = true\nautostart = true\n",
    );
    env.run(&["store", "status"]);

    // Claude Code: 30,000 chars inline, the rest only in the persisted file.
    let claude = format!("echo {}", marker());
    let whole: String = (0..2000).map(|n| format!("claude line {n}\n")).collect();
    let persisted = env.home().join("tmp/persisted.txt");
    std::fs::write(&persisted, &whole).unwrap();
    hook(
        &env,
        "claude-code",
        &serde_json::json!({
            "hook_event_name": "PreToolUse", "tool_name": "Bash", "tool_use_id": "toolu_1",
            "tool_input": {"command": claude, "description": "print a lot"},
        }),
    );
    hook(
        &env,
        "claude-code",
        &serde_json::json!({
            "hook_event_name": "PostToolUse", "tool_name": "Bash", "tool_use_id": "toolu_1",
            "tool_input": {"command": claude},
            "tool_response": {
                "stdout": &whole[..100], "stderr": "", "exitCode": 0,
                "persistedOutputPath": persisted,
            },
        }),
    );

    // Codex: the model-facing text, as a bare string.
    let codex = format!("echo {}", marker());
    hook(
        &env,
        "codex",
        &serde_json::json!({
            "hook_event_name": "PreToolUse", "tool_name": "Bash", "tool_use_id": "call_1",
            "tool_input": {"command": codex},
        }),
    );
    hook(
        &env,
        "codex",
        &serde_json::json!({
            "hook_event_name": "PostToolUse", "tool_name": "Bash", "tool_use_id": "call_1",
            "tool_input": {"command": codex},
            "tool_response": "Total output lines: 1\n\ncodex says hi\n",
        }),
    );

    let mut client = HistoryClient::new(env.socket()).await.unwrap();
    let text = stored(&mut client, &id_of(&env, &claude)).await;
    assert_eq!(text, whole.trim_end_matches('\n'), "the file, not the inline cut, is stored");
    let text = stored(&mut client, &id_of(&env, &codex)).await;
    assert_eq!(text, "Total output lines: 1\n\ncodex says hi");

    let _ = Process::spawn(env.atuin(&["daemon", "stop"])).try_wait();
}
