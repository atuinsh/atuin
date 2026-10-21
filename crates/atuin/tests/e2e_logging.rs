//! Where logs go: the shared log file, and user messages on stderr.

#![cfg(unix)]

mod common;

use std::fs;
use std::process::Output;

use common::{FreshEnv, Process, SESSION};
use rstest::{fixture, rstest};

#[fixture]
fn env() -> FreshEnv {
    FreshEnv::new()
}

impl FreshEnv {
    /// List the history, which opens the history database, with `ATUIN_LOG` set.
    fn list_history(&self, atuin_log: &str) -> Output {
        let mut command = self.atuin(&["history", "list"]);
        command.env("ATUIN_LOG", atuin_log).env("ATUIN_SESSION", SESSION);
        let out = Process::spawn(command).wait();
        assert!(out.status.success(), "listing failed: {}", String::from_utf8_lossy(&out.stderr));
        out
    }
}

#[rstest]
fn cli_logs_to_one_tagged_file(env: FreshEnv) {
    let out = env.list_history("debug");

    let files = env.log_files();
    let [file] = &files[..] else {
        panic!("expected one log file, found {files:?}");
    };
    let name = file.file_name().unwrap().to_str().unwrap();
    let date = name.strip_prefix("atuin.").and_then(|rest| rest.strip_suffix(".log"));
    assert!(date.is_some_and(|date| date.len() == "2026-10-20".len()), "{name}");

    let log = fs::read_to_string(file).unwrap();
    assert!(log.contains("opening sqlite database"), "{log}");
    assert!(log.lines().all(|line| line.starts_with("process=cli pid=")), "{log}");

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stderr.contains("opening sqlite database"), "{stderr}");
    assert!(!stderr.contains("level="), "{stderr}");
}

#[rstest]
fn atuin_log_off_writes_no_file(env: FreshEnv) {
    env.list_history("off");

    assert_eq!(env.log_files(), Vec::<std::path::PathBuf>::new());
}

#[rstest]
fn unwritable_log_dir_warns_and_continues(env: FreshEnv) {
    fs::create_dir_all(env.atuin_home()).unwrap();
    fs::write(env.logs_dir(), "").unwrap();

    let out = env.list_history("debug");

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("warning: couldn't open the log file"), "{stderr}");
}

/// Commands the shell runs on every prompt or keystroke don't pay for opening the log file.
#[rstest]
#[case::init(&["init", "zsh"])]
#[case::uuid(&["uuid"])]
#[case::autosuggest(
    &["search", "--cmd-only", "--limit", "1", "--search-mode", "prefix", "--autosuggest"]
)]
#[case::history_start_hook(&["history", "start", "--hook", "--", "ls"])]
fn hot_paths_create_no_log_file(env: FreshEnv, #[case] args: &[&str]) {
    let mut command = env.atuin(args);
    command.env("ATUIN_LOG", "debug").env("ATUIN_SESSION", SESSION);
    // A search with no match exits non-zero, so only a rejected flag counts as a failure: it
    // would also leave no log file.
    let out = Process::spawn(command).wait();

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stderr.contains("unexpected argument"), "{stderr}");
    assert_eq!(env.log_files(), Vec::<std::path::PathBuf>::new());
}

/// A config that still sets `[logs]` hears about it from commands with stderr to spare.
#[rstest]
#[case::explicit_command(&["history", "list"], true)]
#[case::init(&["init", "zsh"], false)]
fn legacy_logs_table_notice(env: FreshEnv, #[case] args: &[&str], #[case] shown: bool) {
    env.write_config("[logs]\nlevel = \"debug\"\n");
    let mut command = env.atuin(args);
    command.env("ATUIN_SESSION", SESSION);
    let out = Process::spawn(command).wait();

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stderr}");
    let noticed = stderr.contains("the [logs] section in your config is no longer used");
    assert_eq!(noticed, shown, "{stderr}");
}

#[rstest]
fn user_warning_reaches_stderr_without_ansi(env: FreshEnv) {
    env.write_config("[theme]\nname = \"does-not-exist\"\n");
    let mut command = env.atuin(&["search", "--limit", "1", "zzz"]);
    command.env("ATUIN_SESSION", SESSION);
    // Search loads the theme; it exits non-zero only because nothing matches.
    let out = Process::spawn(command).wait();

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("warning: could not load theme does-not-exist"), "{stderr}");
    assert!(!stderr.contains('\x1b'), "{stderr}");
}

#[cfg(feature = "daemon")]
#[rstest]
#[tokio::test]
async fn daemon_and_cli_share_the_file(env: FreshEnv) {
    use atuin_daemon::client::HistoryClient;
    use common::TIMEOUT;

    let mut start = env.atuin(&["daemon", "start", "--show-logs"]);
    start.env("ATUIN_LOG", "debug");
    let mut daemon = Process::spawn(start);
    tokio::time::timeout(TIMEOUT, async {
        loop {
            assert!(
                daemon.child.try_wait().unwrap().is_none(),
                "daemon exited during startup: {}",
                daemon.logs()
            );
            if let Ok(mut client) = HistoryClient::new(env.socket()).await
                && let Ok(status) = client.status().await
                && status.healthy
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("daemon never became healthy");

    env.list_history("debug");
    let stop = Process::spawn(env.atuin(&["daemon", "stop"])).wait();
    assert!(stop.status.success(), "{}", String::from_utf8_lossy(&stop.stderr));
    drop(daemon);

    let log: String =
        env.log_files().iter().map(|file| fs::read_to_string(file).unwrap()).collect();
    assert!(log.lines().any(|line| line.starts_with("process=daemon pid=")), "{log}");
    assert!(log.lines().any(|line| line.starts_with("process=cli pid=")), "{log}");
    assert!(log.lines().all(|line| line.starts_with("process=")), "{log}");
}
