//! Output capture must never store the output of a command `history_filter` or `[output]
//! command_filter` excludes.
//!
//! The two take different routes. A `history_filter` match is kept out of the store only because
//! `atuin history start` returns no id for it, so `$ATUIN_HISTORY_ID` stays empty and the shell
//! integration omits the OSC 133 markers the pty proxy needs to report a capture at all. A
//! `command_filter` match is recorded and captured as usual, and the daemon drops its output on
//! arrival. Both span the CLI, the shell integration, the proxy and the daemon, so this drives all
//! four and checks the property they exist to provide.

#![cfg(all(unix, feature = "daemon", feature = "pty-proxy"))]

mod common;

#[allow(dead_code, reason = "the pty harness is shared with the other e2e test binaries")]
#[path = "common/pty.rs"]
mod pty;
#[allow(dead_code, reason = "only `PROMPT` is needed here; `pty` reaches for it by path")]
#[path = "common/shell.rs"]
mod shell;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use atuin_client::history::HistoryId;
use atuin_common::range::PyStyleIdxRange;
use atuin_daemon::client::HistoryClient;
use common::{FreshEnv, Process, SESSION, TIMEOUT, marker, output};
use pty::PtyShell;
use rstest::rstest;

/// Everything the daemon has ever been told to store history for, as `(id, command)`.
fn recorded(env: &FreshEnv) -> Vec<(HistoryId, String)> {
    let mut list = env.atuin(&["history", "list", "--format", "{uuid}\t{command}"]);
    list.env("ATUIN_SESSION", SESSION);
    output(list)
        .lines()
        .filter_map(|line| line.split_once('\t'))
        .map(|(id, command)| (id.parse().expect("history list prints valid ids"), command.into()))
        .collect()
}

/// Every stored capture the daemon can be asked for, as `(command, output)`.
async fn captured(env: &FreshEnv, client: &mut HistoryClient) -> Vec<(String, String)> {
    let mut captured = Vec::new();
    for (id, command) in recorded(env) {
        // `(0, -1)` is the whole capture, however many lines it turned out to be.
        let whole = vec![PyStyleIdxRange::new(0, -1)];
        if let Some(response) = client.get_command_output(id, whole).await.unwrap() {
            let output: String =
                response.chunks.iter().map(|chunk| chunk.content.as_str()).collect();
            captured.push((command, output));
        }
    }
    captured
}

/// `bash`, honouring the same override and skip rules as the other shell tests.
fn bash() -> Option<PathBuf> {
    let found = match std::env::var_os("ATUIN_E2E_BASH") {
        Some(path) => Some(PathBuf::from(path)),
        None => std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
            .map(|dir| dir.join("bash"))
            .find(|path| path.is_file()),
    };
    if found.is_none() {
        assert!(std::env::var_os("ATUIN_E2E_REQUIRE_SHELLS").is_none(), "missing bash");
        eprintln!("skipping: missing bash");
    }
    found
}

/// `config` excludes commands starting `echo filtered-`; `in_history` is whether it leaves them in
/// history all the same.
#[rstest]
#[case::history_filter("history_filter = [\"^echo filtered-\"]\n[output]\n", false)]
#[case::output_command_filter("[output]\ncommand_filter = [\"^echo filtered-\"]\n", true)]
#[tokio::test]
async fn filtered_commands_leave_no_captured_output(
    #[case] config: &str,
    #[case] in_history: bool,
) {
    let Some(bash) = bash() else {
        return;
    };

    let first = format!("kept-first-{}", marker());
    let filtered = format!("filtered-{}", marker());
    let last = format!("kept-last-{}", marker());

    let env = FreshEnv::new();
    env.write_config(&format!(
        "{config}enabled = true\nmax_output_size = \"1MB\"\nsync = false\nmax_disk_usage = \
         \"unlimited\"\n[daemon]\nenabled = true\nautostart = false\n",
    ));
    // Finish migrations before the daemon and the shell hooks race for the databases.
    env.run(&["store", "status"]);
    std::fs::write(
        env.home().join(".bashrc"),
        "eval \"$(atuin init bash)\"\nPS1=\"E2E_PROMPT> \"\n",
    )
    .unwrap();

    // The capture sink connects to the daemon once, when the proxy starts, so the daemon has to be
    // serving before the shell does.
    let daemon = Process::spawn(env.atuin(&["daemon", "start", "--show-logs"]));
    tokio::time::timeout(TIMEOUT, async {
        loop {
            if let Ok(mut client) = HistoryClient::new(env.socket()).await
                && let Ok(status) = client.status().await
                && status.healthy
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("daemon never became healthy: {}", daemon.logs()));

    let proxy = env.home().join("bin/atuin");
    let args = ["pty-proxy".to_string(), "--shell".to_string(), bash.display().to_string()];
    let pty = PtyShell::spawn(&proxy, &args, &env, &BTreeMap::new());
    pty.wait_for_prompt();

    // The filtered command runs between two kept ones: its output has to be absent from the store
    // *and* from its neighbours' captures, which are rendered from the same screen.
    for command in [&first, &filtered, &last] {
        pty.send_line(&format!("echo {command}"));
        pty.wait_for_line(command);
    }
    // A capture is only reported once the *next* prompt starts, so the last command needs one more
    // round trip before every capture that will ever exist has been sent.
    pty.send_line("true");

    let mut client = HistoryClient::new(env.socket()).await.unwrap();

    // Both the history entry and the capture reach the daemon asynchronously, so wait for the
    // *last* command's capture rather than assuming it has landed. The proxy hands captures to a
    // single ordered channel, so once that one is stored, anything the filtered command might have
    // produced is stored too -- there is nothing left in flight to race with below.
    //
    // Waiting for it is also the positive control: capture really is running in this test, so the
    // assertions after it have teeth instead of passing vacuously.
    //
    // `captured` only looks up ids already in history, and `atuin history end` runs in the
    // background, so a filtered command meant to stay in history is waited for too -- before the
    // captures are read, or its output could be stored and still go unseen.
    let is_filtered_recorded =
        || recorded(&env).iter().any(|(_, command)| command.contains(&filtered));
    let deadline = Instant::now() + TIMEOUT;
    let captured = loop {
        let filtered_recorded = is_filtered_recorded();
        let captured = captured(&env, &mut client).await;
        if captured.iter().any(|(_, output)| output.contains(&last))
            && (filtered_recorded || !in_history)
        {
            break captured;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for the last command's captured output and the filtered command's \
             history entry; captured: {captured:#?}",
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    };

    assert_eq!(is_filtered_recorded(), in_history, "whether the filtered command is in history");

    // Captures are keyed by history id, and the ids above are every id the daemon ever issued, so
    // this covers the whole store.
    assert!(
        !captured.iter().any(|(_, output)| output.contains(&filtered)),
        "the filtered command's output was captured: {captured:#?}",
    );
}
