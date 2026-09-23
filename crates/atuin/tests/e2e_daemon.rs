//! Daemon startup, persistence, and restart tests.

#![cfg(all(unix, feature = "daemon"))]
#![allow(clippy::disallowed_methods, reason = "tests may use std::fs for fixtures")]

mod common;

use std::time::Duration;

use atuin_daemon::client::HistoryClient;
use common::{FreshEnv, Process, SESSION, TIMEOUT, marker, output, wait_until};
use rstest::{fixture, rstest};

struct Daemon {
    foreground: Option<Process>,
    env: FreshEnv,
}

#[fixture]
fn daemon() -> Daemon {
    Daemon {
        foreground: None,
        env: FreshEnv::new(),
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = Process::spawn(self.env.atuin(&["daemon", "stop"])).try_wait();
    }
}

#[rstest]
#[case::foreground(false)]
#[case::autostart(true)]
#[tokio::test]
async fn fresh_daemon_serves_history(
    mut daemon: Daemon,
    #[case] autostart: bool,
    #[values(1, 4)] writers: usize,
) {
    daemon.env.write_config(&format!(
        "local_timeout = 15\n[daemon]\nenabled = true\nautostart = {autostart}\n"
    ));
    assert!(!daemon.env.data_dir().join("key").exists());
    assert!(!daemon.env.data_dir().join("history.db").exists());
    assert!(!daemon.env.socket().exists());

    // Check socket and startup-lock reuse after shutdown.
    for _ in 0..2 {
        if !autostart {
            daemon.foreground =
                Some(Process::spawn(daemon.env.atuin(&["daemon", "start", "--show-logs"])));
            tokio::time::timeout(TIMEOUT, async {
                loop {
                    let process = daemon.foreground.as_mut().unwrap();
                    assert!(
                        process.child.try_wait().unwrap().is_none(),
                        "daemon exited during startup: {}",
                        process.logs()
                    );
                    if let Ok(mut client) = HistoryClient::new(daemon.env.socket()).await
                        && let Ok(status) = client.status().await
                        && status.healthy
                    {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .expect("daemon never became healthy");
        }

        // Spawn all writers first to exercise concurrent autostart.
        let commands: Vec<_> = (0..writers).map(|_| format!("echo {}", marker())).collect();
        let processes: Vec<_> = commands
            .iter()
            .map(|command| {
                let mut start = daemon.env.atuin(&["history", "start", "--", command]);
                start.env("ATUIN_SESSION", SESSION);
                Process::spawn(start)
            })
            .collect();
        let mut ids = Vec::new();
        for process in processes {
            let out = process.wait();
            assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
            let id = String::from_utf8(out.stdout).unwrap().trim().to_owned();
            assert!(
                id.parse::<atuin_client::history::HistoryId>().is_ok(),
                "invalid history ID: {id:?}"
            );
            daemon.env.run(&["history", "end", "--exit", "7", &id]);
            ids.push(id);
        }

        let mut client = tokio::time::timeout(TIMEOUT, HistoryClient::new(daemon.env.socket()))
            .await
            .unwrap()
            .unwrap();
        let status = tokio::time::timeout(TIMEOUT, client.status()).await.unwrap().unwrap();
        assert!(status.healthy);
        assert_eq!(status.version, env!("CARGO_PKG_VERSION"));
        let pidfile =
            std::fs::read_to_string(daemon.env.data_dir().join("atuin-daemon.pid")).unwrap();
        assert_eq!(pidfile.lines().next().unwrap(), status.pid.to_string());
        if let Some(process) = &daemon.foreground {
            assert_eq!(process.child.id(), status.pid);
        }
        assert!(daemon.env.data_dir().join("key").is_file());

        let expected: Vec<_> =
            ids.iter().zip(&commands).map(|(id, command)| format!("{id}\t7\t{command}")).collect();
        wait_until("daemon history persisted", || {
            let mut list =
                daemon.env.atuin(&["history", "list", "--format", "{uuid}\t{exit}\t{command}"]);
            list.env("ATUIN_SESSION", SESSION);
            let history = output(list);
            expected.iter().all(|row| history.lines().any(|line| line == row))
        });
        assert_eq!(
            tokio::time::timeout(TIMEOUT, client.status()).await.unwrap().unwrap().pid,
            status.pid
        );
        assert!(daemon.env.run(&["daemon", "stop"]).contains("Daemon stopped"));
        wait_until("daemon socket removed", || !daemon.env.socket().exists());
    }
}

/// Ending a command must not autostart the daemon: a fresh daemon has no record of a command it
/// didn't see start, and a daemon spawned while systemd tears down the terminal's scope would miss
/// the SIGTERM sweep and stall shutdown until SIGKILL (#4225).
#[rstest]
#[case::end(0)]
#[case::cancel(1)]
#[tokio::test]
async fn history_end_does_not_autostart_daemon(daemon: Daemon, #[case] exit: i64) {
    daemon.env.write_config(
        "local_timeout = 15\nstore_failed = false\n[daemon]\nenabled = true\nautostart = true\n",
    );

    let mut start = daemon.env.atuin(&["history", "start", "--", &format!("echo {}", marker())]);
    start.env("ATUIN_SESSION", SESSION);
    let id = output(start).trim().to_owned();
    assert!(daemon.env.socket().exists(), "history start should autostart the daemon");

    assert!(daemon.env.run(&["daemon", "stop"]).contains("Daemon stopped"));
    wait_until("daemon socket removed", || !daemon.env.socket().exists());

    let end = daemon.env.atuin(&["history", "end", "--exit", &exit.to_string(), "--", &id]);
    let _ = Process::spawn(end).wait();

    assert!(!daemon.env.socket().exists(), "history end autostarted the daemon");
    assert!(daemon.env.run(&["daemon", "status"]).contains("Daemon is not running"));
}

/// With the daemon enabled but not running (and not allowed to start), history is saved locally
/// instead of being dropped (#3866). The fallback warns, unless `--hook` silences logging.
#[rstest]
#[tokio::test]
async fn history_falls_back_to_local_when_daemon_not_running(
    daemon: Daemon,
    #[values(false, true)] hook: bool,
) {
    daemon.env.write_config("local_timeout = 15\n[daemon]\nenabled = true\nautostart = false\n");
    let command = format!("echo {}", marker());
    let hook_arg: &[&str] = if hook {
        &["--hook"]
    } else {
        &[]
    };

    let mut start =
        daemon.env.atuin(&[&["history", "start"], hook_arg, &["--", &command]].concat());
    start.env("ATUIN_SESSION", SESSION);
    let out = Process::spawn(start).wait();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let start_stderr = String::from_utf8(out.stderr).unwrap();
    let id = String::from_utf8(out.stdout).unwrap().trim().to_owned();

    let end =
        daemon.env.atuin(&[&["history", "end", "--exit", "7"], hook_arg, &["--", &id]].concat());
    let out = Process::spawn(end).wait();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let end_stderr = String::from_utf8(out.stderr).unwrap();

    assert!(!daemon.env.socket().exists(), "the daemon should not have been started");
    for (stderr, verb) in [(start_stderr, "start"), (end_stderr, "end")] {
        let warned = stderr.contains(&format!("failed to {verb} history via the daemon"));
        assert_eq!(warned, !hook, "unexpected `history {verb}` stderr: {stderr}");
    }

    let mut list = daemon.env.atuin(&["history", "list", "--format", "{uuid}\t{exit}\t{command}"]);
    list.env("ATUIN_SESSION", SESSION);
    let expected = format!("{id}\t7\t{command}");
    assert!(output(list).lines().any(|line| line == expected), "history was not saved locally");
}
