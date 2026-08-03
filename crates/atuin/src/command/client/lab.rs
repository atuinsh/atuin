#[cfg(unix)]
use std::process::{Command, Stdio};
#[cfg(unix)]
use std::time::{Duration, Instant};

use clap::Subcommand;
#[cfg(unix)]
use eyre::bail;
use eyre::{Result, WrapErr};
use url::Url;

use atuin_client::settings::Settings;

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// Share your terminal with others (experimental)
    Share {
        /// Allow anyone with the link to send keystrokes to your shell
        #[arg(long)]
        write: bool,

        /// Skip the confirmation prompt (the warning is still printed)
        #[arg(long)]
        yes: bool,

        /// Share the session already running in this terminal (requires an
        /// atuin pty-proxy owning it; see `atuin pty-proxy init`)
        #[arg(long, hide = true)]
        active: bool,

        /// Run --active attached to this terminal instead of in the
        /// background (debug): the URL prints to stderr, Ctrl-C stops
        #[arg(long, hide = true, requires = "active")]
        foreground: bool,

        /// Stop the background --active share session
        #[arg(
            long,
            conflicts_with_all = ["write", "yes", "active", "foreground", "url", "internal_daemon"]
        )]
        stop: bool,

        /// Print the join URL of the running background --active share
        #[arg(
            long,
            conflicts_with_all = ["write", "yes", "active", "foreground", "stop", "internal_daemon"]
        )]
        url: bool,

        /// Run as the re-exec'd daemonized child of --active (internal: the
        /// process forks into the background before the runtime is built)
        #[arg(long, hide = true, requires = "active", conflicts_with = "foreground")]
        internal_daemon: bool,
    },
}

impl Cmd {
    /// True when this invocation is the re-exec'd `--internal-daemon` child.
    ///
    /// `client.rs` checks this BEFORE building the tokio runtime and forks
    /// there: `fork()` inside a live runtime corrupts its internal state, so
    /// daemonizing must never move into the async `run` below.
    #[cfg(unix)]
    pub fn should_daemonize(&self) -> bool {
        match self {
            Self::Share {
                internal_daemon, ..
            } => *internal_daemon,
        }
    }

    /// Async because the Hub credential accessor is async. Everything that
    /// needs `await` happens here; `run_share` receives plain data so it never
    /// has to build a nested tokio runtime.
    pub async fn run(self, settings: &Settings) -> Result<()> {
        report_refusal(self.dispatch(settings).await)
    }

    async fn dispatch(self, settings: &Settings) -> Result<()> {
        match self {
            Self::Share {
                write,
                yes,
                active,
                foreground,
                stop,
                url,
                internal_daemon,
            } => {
                #[cfg(unix)]
                {
                    if stop {
                        return stop_share().await;
                    }
                    if url {
                        return print_share_url();
                    }
                    // Plain `--active` backgrounds itself: this foreground
                    // process is only the launcher. `--foreground` and the
                    // re-exec'd child run the session in-process below.
                    if active && !foreground && !internal_daemon {
                        return spawn_background_share(write, yes).await;
                    }
                }
                #[cfg(not(unix))]
                let _ = (stop, url);

                let hub_url = lab_ws_url(settings)?;
                let api_token = lab_api_token(settings).await?;
                // `atuin_lab_share::Error` converts to `eyre::Report` via the
                // blanket `From<E: std::error::Error>`.
                Ok(atuin_lab_share::run_share(atuin_lab_share::ShareOptions {
                    // Never implied by --internal-daemon: the legitimate
                    // child is always spawned WITH --yes (its parent already
                    // confirmed on the interactive terminal), so a hand-typed
                    // --internal-daemon without it fails closed at the
                    // confirmation gate — no flag may open a promptless path
                    // to the hub on its own.
                    yes,
                    write,
                    active,
                    foreground,
                    internal_daemon,
                    hub_url,
                    api_token,
                })
                .await?)
            }
        }
    }
}

/// Reports the `--active` refusal as the UX surface it is.
///
/// Every rung of the detection ladder failing is a normal, expected outcome
/// with instructions attached, not a program fault. Returned as an `Err` it
/// would reach eyre's reporter, which appends a `Location:` trailer pointing
/// into this file — reading as a crash and burying the instructions. So print
/// it plainly and exit non-zero instead. Every other error keeps the standard
/// reporting.
///
/// Exiting without unwinding is safe here specifically: the refusal is raised
/// before any session, pidfile lock, spawned child, or raw-mode terminal
/// exists, so there is nothing whose `Drop` must run.
fn report_refusal(result: Result<()>) -> Result<()> {
    if let Some(refusal @ atuin_lab_share::Error::ActiveShareUnsupported) = result
        .as_ref()
        .err()
        .and_then(eyre::Report::downcast_ref::<atuin_lab_share::Error>)
    {
        eprintln!("error: {refusal}");
        std::process::exit(1);
    }
    result
}

/// How long the spawning parent waits for the daemonized child to publish
/// its join URL: must cover the child's 20s hub connect timeout with
/// headroom.
#[cfg(unix)]
const SPAWN_URL_BUDGET: Duration = Duration::from_secs(25);

/// Grace before the parent treats a free pidfile lock as "the child died":
/// daemonize's fork plus runtime startup need a moment before the child
/// holds the lock.
#[cfg(unix)]
const SPAWN_LOCK_GRACE: Duration = Duration::from_secs(3);

/// The parent's poll cadence while waiting for the URL file.
#[cfg(unix)]
const SPAWN_POLL: Duration = Duration::from_millis(100);

/// How long `--stop` waits for the SIGTERM'd child to release the lock.
#[cfg(unix)]
const STOP_TIMEOUT: Duration = Duration::from_secs(10);

#[cfg(unix)]
const NO_ACTIVE_SHARE: &str = "no active share session.";
#[cfg(unix)]
const SHARING_STOPPED: &str = "sharing stopped.";
#[cfg(unix)]
const STOP_TIMED_OUT: &str =
    "sharing did not stop within 10s; the share process may still be running";
#[cfg(unix)]
const URL_STILL_CONNECTING: &str = "the share is still connecting; try again shortly";
#[cfg(unix)]
const SPAWN_TIMED_OUT: &str = "timed out waiting for the background share to connect; run \
     `atuin lab share --active --foreground` to debug, or `atuin lab share --stop` to clean up";
#[cfg(unix)]
const SPAWN_CHILD_DIED: &str = "the background share exited before publishing a URL; run \
     `atuin lab share --active --foreground` to see why";

/// The success copy after the background share published its URL. Pure
/// ASCII, byte-pinned by a test.
#[cfg(unix)]
fn attach_copy(url: &str) -> String {
    format!(
        "Sharing this session at: {url}\n\
         Run `atuin lab share --stop` to end sharing. `atuin lab share --url` reprints the link.\n\
         This session has no warning bar; viewers stay connected until you stop or the shell exits."
    )
}

/// The parent half of `--active`: confirm on this (interactive) terminal,
/// re-exec ourselves as a daemonized child, and wait for it to publish the
/// join URL.
///
/// The child owns the session; this process exits as soon as the URL is
/// known (exit 0) or the child demonstrably failed (exit 1). Liveness while
/// waiting is the pidfile lock: held means the child is up and connecting; a
/// free lock after a startup grace means it died — and its stdio is null, so
/// the pointer to `--foreground` is the debugging path.
#[cfg(unix)]
async fn spawn_background_share(write: bool, yes: bool) -> Result<()> {
    use atuin_lab_share::lifecycle;

    // Ladder + warning + [y/N] prompt run HERE, on the interactive terminal.
    // The child re-runs the ladder in its own environment but never prompts:
    // it is spawned with --yes because its stdio is null.
    if !atuin_lab_share::preflight_active_share(write, yes)? {
        return Ok(());
    }

    let pidfile = lifecycle::pidfile_path();
    let url_file = lifecycle::url_file_path();

    // One active share per user. Refusing here (not only in the child) fails
    // the common collision fast, before a process is spawned; the child's
    // pidfile lock stays the authoritative arbiter.
    if lifecycle::probe_lock(&pidfile)? == lifecycle::LockState::Held {
        return Err(atuin_lab_share::Error::ShareAlreadyRunning.into());
    }

    // The spawn id ties this launch to the URL file its child writes: the
    // child records it as the file's owner line, and the poll below accepts
    // no other. A stale file from a dead session, or one written by a share
    // racing this launch, can therefore never be printed as OUR success —
    // and no lock-free stale-file cleanup is needed here (the child removes
    // leftovers itself, right after winning the pidfile lock; deleting from
    // this side could race a concurrent launch and strand the winner's URL).
    let spawn_id = atuin_common::utils::uuid_v7().as_simple().to_string();

    let exe = std::env::current_exe().wrap_err("could not locate atuin executable")?;
    let mut cmd = Command::new(exe);
    cmd.args(["lab", "share", "--active", "--internal-daemon", "--yes"]);
    if write {
        cmd.arg("--write");
    }
    cmd.env(lifecycle::SPAWN_ID_ENV, &spawn_id);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = cmd
        .spawn()
        .wrap_err("failed to spawn the background share process")?;

    let start = Instant::now();
    loop {
        // The spawned process exits almost immediately (daemonize re-forks
        // the real session out of it); reap it so it never lingers as a
        // zombie while we poll.
        let _ = child.try_wait();

        if lifecycle::read_url_file_owner(&url_file).as_deref() == Some(spawn_id.as_str())
            && let Some(url) = lifecycle::read_url_file(&url_file)
        {
            println!("{}", attach_copy(&url));
            return Ok(());
        }
        if start.elapsed() >= SPAWN_URL_BUDGET {
            bail!(SPAWN_TIMED_OUT);
        }
        if start.elapsed() >= SPAWN_LOCK_GRACE
            && lifecycle::probe_lock(&pidfile)? == lifecycle::LockState::Free
        {
            bail!(SPAWN_CHILD_DIED);
        }
        tokio::time::sleep(SPAWN_POLL).await;
    }
}

/// `--stop`: SIGTERM the daemonized child and wait for the pidfile lock to
/// confirm it is gone. The child's own teardown removes the URL file and
/// tells the hub the session ended; the removal here is the backstop for a
/// child that died mid-teardown.
#[cfg(unix)]
async fn stop_share() -> Result<()> {
    use atuin_lab_share::lifecycle;

    let pidfile = lifecycle::pidfile_path();
    let url_file = lifecycle::url_file_path();

    if lifecycle::probe_lock(&pidfile)? == lifecycle::LockState::Free {
        // Nothing running: clean whatever a dead session left behind.
        lifecycle::remove_url_file(&url_file);
        let _ = std::fs::remove_file(&pidfile);
        eprintln!("{NO_ACTIVE_SHARE}");
        std::process::exit(1);
    }

    let Some(pid) = lifecycle::read_pidfile_pid(&pidfile) else {
        bail!(
            "a share is running but its pidfile at {} is unreadable",
            pidfile.display()
        );
    };

    // Remember whose URL file we are about to orphan, so the backstop
    // removal below can tell it apart from a NEW share's file.
    let stopped_owner = lifecycle::read_url_file_owner(&url_file);
    let stopped_url = lifecycle::read_url_file(&url_file);

    // SIGTERM: the headless session's signal arm turns this into a graceful
    // teardown (tap detach — never the user's shell — and `End` to the hub).
    // Shelling out to `kill` follows daemon.rs's `force_cleanup` precedent.
    let _ = Command::new("kill")
        .args(["-TERM", &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();

    if !lifecycle::wait_for_lock_release(&pidfile, STOP_TIMEOUT).await {
        bail!(STOP_TIMED_OUT);
    }
    // Backstop for a child that died mid-teardown — but only while the file
    // is still the stopped session's: a share launched in this very instant
    // may already own it (and it cleaned any stale file itself, behind the
    // pidfile lock), so an unconditional removal here could strand the new
    // session with no URL file at all.
    if lifecycle::read_url_file_owner(&url_file) == stopped_owner
        && lifecycle::read_url_file(&url_file) == stopped_url
    {
        lifecycle::remove_url_file(&url_file);
    }
    println!("{SHARING_STOPPED}");
    Ok(())
}

/// `--url`: reprint the running share's join URL from the URL file. Only
/// meaningful while the pidfile lock says a share is alive — a leftover URL
/// file with a free lock is a dead session, not a link worth printing.
#[cfg(unix)]
fn print_share_url() -> Result<()> {
    use atuin_lab_share::lifecycle;

    if lifecycle::probe_lock(&lifecycle::pidfile_path())? == lifecycle::LockState::Held {
        match lifecycle::read_url_file(&lifecycle::url_file_path()) {
            Some(url) => {
                println!("{url}");
                Ok(())
            }
            // Lock held but no URL yet: the child is still connecting.
            None => bail!(URL_STILL_CONNECTING),
        }
    } else {
        eprintln!("{NO_ACTIVE_SHARE}");
        std::process::exit(1);
    }
}

/// Resolve the Hub websocket base URL from settings, honouring self-hosted Hubs
/// via `Settings::hub_endpoint()`.
///
/// `ATUIN_LAB_HUB_URL` is parsed **as given** and its scheme is never rewritten:
/// local development runs against a plain-HTTP dev hub as `ws://localhost:4000`,
/// and upgrading that to `wss` would fail the handshake. The scheme is only
/// derived (http→ws, https→wss) when the override is absent.
fn lab_ws_url(settings: &Settings) -> Result<Url> {
    if let Ok(u) = std::env::var("ATUIN_LAB_HUB_URL") {
        return Url::parse(&u).wrap_err("ATUIN_LAB_HUB_URL is not a valid URL");
    }
    let mut url = settings.hub_endpoint();
    let ws_scheme = if url.scheme() == "http" { "ws" } else { "wss" };
    let _ = url.set_scheme(ws_scheme);
    Ok(url)
}

/// The **Hub** session token — *not* the sync/`atuin-server` session token.
///
/// Do not "simplify" this to `Settings::session_token()`: they are different
/// credentials in different storage slots. Hub tokens are minted by
/// `AtuinHub.Accounts.create_api_token_for/2` with an **`atapi_` prefix** (which
/// is exactly how atuin-client's token-slot logic tells hub tokens from sync
/// tokens), and the hub authenticates this socket with
/// `Accounts.find_api_token_by(code:)` against its `api_tokens` table. A sync
/// token will never match, so using it fails 100 % of joins.
///
/// `ATUIN_LAB_HUB_TOKEN` overrides it for local development against a hub with a
/// hand-minted token (see Plan C's end-to-end task).
async fn lab_api_token(settings: &Settings) -> Result<String> {
    if let Ok(t) = std::env::var("ATUIN_LAB_HUB_TOKEN") {
        return Ok(t);
    }
    settings
        .hub_session_token()
        .await
        .wrap_err("not logged in to Atuin Hub -- run `atuin login` first")
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// The attach copy the parent prints on success is user-visible and
    /// byte-frozen once shipped: pin it exactly, and keep it pure ASCII.
    #[test]
    fn attach_copy_is_pinned() {
        let copy = attach_copy("https://hub.example/s/abc#key");
        assert_eq!(
            copy,
            "Sharing this session at: https://hub.example/s/abc#key\n\
             Run `atuin lab share --stop` to end sharing. `atuin lab share --url` reprints the link.\n\
             This session has no warning bar; viewers stay connected until you stop or the shell exits."
        );
        assert!(copy.is_ascii());
    }

    /// The `--stop`/`--url` copy is likewise frozen.
    #[test]
    fn lifecycle_copy_is_pinned() {
        assert_eq!(NO_ACTIVE_SHARE, "no active share session.");
        assert_eq!(SHARING_STOPPED, "sharing stopped.");
        for msg in [
            NO_ACTIVE_SHARE,
            SHARING_STOPPED,
            STOP_TIMED_OUT,
            URL_STILL_CONNECTING,
            SPAWN_TIMED_OUT,
            SPAWN_CHILD_DIED,
        ] {
            assert!(msg.is_ascii(), "{msg:?} must stay pure ASCII");
        }
    }
}
