//! Process termination on SIGTERM and SIGINT.

use std::io;
use std::time::Duration;

/// How long a graceful shutdown may take before the process is forced to exit.
pub const GRACE: Duration = Duration::from_secs(5);

/// Calls `on_signal` on the first termination signal, then force-exits the process if it is still
/// running `grace` later or when a second signal arrives.
///
/// Signals are handled off the async runtime, so a wedged runtime can't swallow them.
#[cfg(unix)]
pub fn install(grace: Duration, on_signal: impl FnOnce() + Send + 'static) -> io::Result<()> {
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    use signal_hook::consts::{SIGINT, SIGTERM};
    use signal_hook::flag;
    use signal_hook::iterator::Signals;

    const SIGNALS: [i32; 2] = [SIGTERM, SIGINT];

    let armed = Arc::new(AtomicBool::new(false));
    for signal in SIGNALS {
        // Registered before the flag is set, so only a second signal exits.
        flag::register_conditional_shutdown(signal, 128 + signal, Arc::clone(&armed))?;
        flag::register(signal, Arc::clone(&armed))?;
    }

    let mut signals = Signals::new(SIGNALS)?;
    std::thread::Builder::new().name("atuin-daemon-signals".into()).spawn(move || {
        let Some(signal) = signals.forever().next() else {
            return;
        };
        tracing::info!(signal, "received shutdown signal");
        on_signal();
        std::thread::sleep(grace);
        tracing::error!(?grace, "graceful shutdown timed out, forcing exit");
        // `_exit`: a stuck `exit()` on the main thread would block another `exit()`.
        signal_hook::low_level::exit(128 + signal);
    })?;
    Ok(())
}

/// Calls `on_signal` on Ctrl+C, then force-exits the process if it is still running `grace` later.
#[cfg(not(unix))]
pub fn install(grace: Duration, on_signal: impl FnOnce() + Send + 'static) -> io::Result<()> {
    tokio::spawn(async move {
        if let Err(e) = tokio::signal::ctrl_c().await {
            tracing::error!("failed to listen for ctrl+c: {e}");
            return;
        }
        tracing::info!("received shutdown signal");
        on_signal();
        std::thread::spawn(move || {
            std::thread::sleep(grace);
            tracing::error!(?grace, "graceful shutdown timed out, forcing exit");
            std::process::exit(1);
        });
    });
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    use rstest::rstest;
    use signal_hook::consts::SIGTERM;
    use signal_hook::low_level::raise;

    use super::install;

    const CHILD: &str = "ATUIN_DAEMON_SHUTDOWN_TEST_CHILD";

    fn wedged_daemon(grace: Duration, signals: usize) -> ! {
        install(grace, || {}).unwrap();
        for _ in 0..signals {
            raise(SIGTERM).unwrap();
            std::thread::sleep(Duration::from_millis(50));
        }
        loop {
            std::thread::park();
        }
    }

    #[rstest]
    #[case::grace_expires(Duration::from_millis(100), 1)]
    #[case::second_signal(Duration::from_secs(60), 2)]
    fn wedged_shutdown_is_forced_to_exit(#[case] grace: Duration, #[case] signals: usize) {
        if std::env::var_os(CHILD).is_some() {
            wedged_daemon(grace, signals);
        }

        // A forced exit ends the whole process, so the wedged daemon is a re-run of this test.
        let test = std::thread::current().name().unwrap().to_owned();
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([test.as_str(), "--exact"])
            .env(CHILD, "1")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if Instant::now() > deadline {
                child.kill().unwrap();
                panic!("wedged daemon was never forced to exit");
            }
            std::thread::sleep(Duration::from_millis(10));
        };

        assert_eq!(status.code(), Some(128 + SIGTERM));
    }
}
