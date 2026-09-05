//! Boot the installed binary without pre-existing databases or keys.

#![cfg(all(unix, feature = "daemon"))]

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
        // The private socket prevents cleanup from reaching another daemon.
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

    // Repeat after shutdown to check that the socket and startup lock can be reused.
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

        // Start all writers before waiting, so autostart must coordinate concurrent hooks.
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
