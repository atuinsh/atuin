//! Whether the daemon is rebuilding the session index the picker reads.
//!
//! During `atuin store rebuild ai-session` (or a purge, or right after the daemon starts) the
//! daemon empties the sidecar and replays it from the record store in the background, so the
//! picker's searches come back partial, or empty. The picker reads the sidecar directly and never
//! waits for the daemon, so it only asks the daemon, now and then and briefly, whether it is
//! rebuilding, and says so beside the results. An unreachable daemon (not running, or not
//! answering in time) is never started, and says nothing.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use atuin_client::settings::Settings;
use atuin_daemon::AiClient;
use tokio::sync::watch;

/// How long a probe may take, connecting included, before the daemon counts as unreachable.
const PROBE_TIMEOUT: Duration = Duration::from_millis(300);

/// The daemon is rebuilding the session index.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rebuilding {
    /// Records replayed, and roughly how many there are to replay.
    pub progress: (u64, u64),
}

impl Rebuilding {
    /// The picker's status line while it lasts.
    #[must_use]
    pub fn status(&self) -> String {
        let (replayed, pending) = self.progress;
        if pending > 0 {
            format!(
                "rebuilding the session index: {replayed} of {pending} records; results may be \
                 incomplete"
            )
        } else {
            "rebuilding the session index; results may be incomplete".to_owned()
        }
    }
}

/// Asks whether the session index is being rebuilt. Tests swap it for one that asks nothing.
#[async_trait]
pub trait RebuildProbe: Send + Sync {
    /// `None` when it isn't, or when nobody can say (the daemon isn't reachable).
    async fn rebuilding(&self) -> Option<Rebuilding>;
}

/// Asks the daemon over its socket, without starting it, giving up after [`PROBE_TIMEOUT`].
pub struct DaemonProbe {
    settings: Settings,
}

impl DaemonProbe {
    #[must_use]
    pub fn new(settings: &Settings) -> Self {
        Self {
            settings: settings.clone(),
        }
    }
}

#[async_trait]
impl RebuildProbe for DaemonProbe {
    async fn rebuilding(&self) -> Option<Rebuilding> {
        let probe = async {
            let mut client = AiClient::from_settings(&self.settings).await.ok()?;
            client.rebuild_status().await.ok()?
        };
        let progress = tokio::time::timeout(PROBE_TIMEOUT, probe).await.ok().flatten()?;
        Some(Rebuilding { progress })
    }
}

/// Ask `probe` at once and then every `every`, until the receiver is dropped: the receiver
/// changes whenever the answer does.
pub fn watch(probe: Arc<dyn RebuildProbe>, every: Duration) -> watch::Receiver<Option<Rebuilding>> {
    let (tx, rx) = watch::channel(None);
    tokio::spawn(async move {
        loop {
            let now = probe.rebuilding().await;
            tx.send_if_modified(|was| {
                let changed = *was != now;
                *was = now;
                changed
            });
            tokio::select! {
                () = tokio::time::sleep(every) => {}
                () = tx.closed() => return,
            }
        }
    });
    rx
}

#[cfg(test)]
mod tests {
    use parking_lot::Mutex;
    use rstest::rstest;

    use super::*;

    /// Answers from a script, then not rebuilding.
    struct Scripted(Mutex<Vec<Option<Rebuilding>>>);

    #[async_trait]
    impl RebuildProbe for Scripted {
        async fn rebuilding(&self) -> Option<Rebuilding> {
            let mut script = self.0.lock();
            if script.is_empty() {
                None
            } else {
                script.remove(0)
            }
        }
    }

    fn at(replayed: u64, pending: u64) -> Rebuilding {
        Rebuilding {
            progress: (replayed, pending),
        }
    }

    #[rstest]
    #[tokio::test]
    async fn the_watch_follows_the_rebuild_until_it_ends() {
        let script = vec![Some(at(1, 10)), Some(at(1, 10)), Some(at(5, 10)), None];
        let mut rx = watch(Arc::new(Scripted(Mutex::new(script))), Duration::from_millis(1));
        let mut seen = Vec::new();
        while seen.last() != Some(&None) {
            tokio::time::timeout(Duration::from_secs(10), rx.changed()).await.unwrap().unwrap();
            seen.push(*rx.borrow_and_update());
        }
        // Only the changes: the repeated answer isn't one.
        assert_eq!(seen, vec![Some(at(1, 10)), Some(at(5, 10)), None]);
    }

    #[rstest]
    #[case::progress(
        at(1200, 5000),
        "rebuilding the session index: 1200 of 5000 records; results may be incomplete"
    )]
    #[case::nothing_to_replay(at(0, 0), "rebuilding the session index; results may be incomplete")]
    fn the_status_says_how_far_it_has_got(#[case] rebuilding: Rebuilding, #[case] want: &str) {
        assert_eq!(rebuilding.status(), want);
    }

    /// With no daemon listening, the probe says nothing, and quickly.
    #[cfg(unix)]
    #[rstest]
    #[tokio::test]
    async fn an_unreachable_daemon_says_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let mut settings = Settings::utc();
        settings.daemon.socket_path = Some(dir.path().join("nobody.sock"));
        settings.daemon.pidfile_path = dir.path().join("atuin-daemon.pid").to_string_lossy().into();
        let started = std::time::Instant::now();
        assert_eq!(DaemonProbe::new(&settings).rebuilding().await, None);
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(!dir.path().join("nobody.sock").exists(), "no daemon was started");
    }
}
