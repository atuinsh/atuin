use std::num::NonZeroUsize;
use std::sync::Arc;

use atuin_client::ai_session::{AiSessionDatabase, AiSessionStore};
use atuin_client::database::Sqlite as HistoryDatabase;
use atuin_client::history::store::HistoryStore;
use atuin_client::record::sqlite_store::SqliteStore;
use atuin_client::settings::Settings;
use atuin_client::settings::watcher::global_settings_watcher;
use atuin_common::sync::BlockingPool;
use eyre::Result;

use crate::grpc::ai::session::pb::ai_session_server::AiSessionServer;
use crate::grpc::history::pb::history_server::HistoryServer;
use crate::session_capture::AiHarnessSessionCapture;

pub mod client;
pub mod components;
pub mod daemon;
pub mod events;
pub mod grpc;
pub(crate) mod history_journal;
mod output_capture;
pub mod pidfile;
pub mod search;
pub mod server;
pub mod session_capture;
mod sync;

// Re-export core daemon types for convenience
// Re-export client helpers
pub use client::{AiClient, HistoryClient};
// Re-export components
pub use components::SearchComponent;
pub use daemon::{AnyComponent, Daemon, DaemonBuilder, DaemonHandle};
pub use events::DaemonEvent;
pub use history_journal::{
    CmdCancelError, CmdDeleteError, CmdEvent, CmdFinishError, CmdRebuildError, FinishedCmd,
    GetCmdInFlightError, HistoryJournal, RegisterOutputError,
};
pub use output_capture::{
    CaptureError, DeleteOutputError, GetOutputError, OutputCaptureEngine, OutputLine, OutputMatch,
};

/// Blocking work running at once in the daemon's [`BlockingPool`]. Tokio's own blocking pool
/// allows 512 threads, past macOS's default soft limit of 256 open files.
const MAX_BLOCKING_WORKERS: NonZeroUsize = NonZeroUsize::new(32).expect("32 is non-zero");

/// The daemon's version (the Atuin version).
///
/// Clients restart a daemon whose version doesn't match theirs; see [`PROTOCOL_VERSION`].
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// TL;DR:
///
///   - If you change the proto files, ensure you use the `reserved` proto keyword for modifying
///     anything related to the `Shutdown` and `Status` RPCs.
///   - Go ham on breaking wire compatibility on every other RPC. **Make sure you bump
///     `PROTOCOL_VERSION`.** The daemon and the client both use this constant.
///
/// # Breaking Wire Compatibility
///
/// Changing this version impacts the grpc version and at first glance risks incompatibility
/// changes. This documentation aims to instill some confidence in making sure this prevents
/// breakages.
///
/// ## Restart Behavior
///
/// First and foremost, let us consider how the client restarts the daemon. Nominally, the client
/// will restart the daemon if the daemon has a mismatched version in the following cases:
///
/// `atuin::command::client::daemon:ready_client` is the function responsible for fetching a valid
/// client. Each **history** client->daemon connection goes through this function, which:
///
///   1. Probes the daemon via the `history.Status` RPC.
///   2. Checks that the response version `VERSION` (ie. the Atuin version) matches that of
///      the client and also that `PROTOCOL_VERSION` matches that of the client.
///   3. If there is a mismatch, one of two things will happen:
///      a) If `daemon.autostart = false`, then we kindly ask the user to restart the daemon and
///      return an error.
///      b) If `daemon.autostart = true`, then we try to restart the daemon. This means one of
///      multiple things:
///      - If `daemon.systemd_socket` is set to false, we tell the user to restart the daemon, and
///        we bail out.
///      - Otherwise, we send a `history.Shutdown` RPC (version unchecked) to the daemon which
///        causes the daemon to shut down. The client then brings it back up, and spinloops until
///        the daemon is back online.
///
///   4. At this point, either:
///      - We were unable to restart the daemon and have told the user to manually restart or
///      - We have restarted the daemon.
///
/// ### Transport Layer
///
/// I would like to note that restarting the daemon causes the UNIX socket file to be completely
/// erased (in the non-systemd path), effectively flushing any messages that are enqueued to the
/// daemon.
///
/// One of the concerns was that the daemon would receive old data, even if both the client **and**
/// the daemon were to be restarted, since stale data could be queued. Luckily, that's not the case!
///
/// TODO(markovejnovic): Is this a concern in the systemd case?
///
/// ### Different Package Managers
///
/// There are many ways to install Atuin and consequently let's analyze which installation method
/// hits which code path.
///
/// #### curl Install Script
///
/// - If `daemon.autostart = true` (default when the daemon is enabled in the setup), the daemon
///   will gracefully be restarted when `PROTOCOL_VERSION` changes. Wire incompatibility is
///   a-ok in this case.
/// - If `daemon.autostart = false`, we will tell the user to restart Atuin.
///
/// #### Homebrew
///
/// <https://raw.githubusercontent.com/Homebrew/homebrew-core/master/Formula/a/atuin.rb> presents
/// the user with a homebrew service. This means the user can run `brew services start atuin` to run
/// a new atuin daemon.
///
/// We will try to restart it via the `Shutdown` RPC. When the daemon attempts to game end, it will
/// terminate, and the client will immediately to try to run another daemon:
///
///   - Either `keep_alive` (on macOS launchd) wins, which means a new version of the daemon is
///     spawned, and managed by launchd.
///   - Or, more likely, the client-spawned daemon wins, which means a new version of the daemon is
///     managed by the client.
///
/// **In effect, if the user updates via brew, the next command we send to the daemon will hit that
/// `PROTOCOL_VERSION` mismatch and the daemon will get restarted, one way or another.**
///
/// The only exception to this rule is if they:
///  - Ran `brew service start atuin`
///  - And also said **no** to the daemon during setup.
///
/// But in that case, we won't even try to talk to the daemon, so a mismatched version doesn't
/// matter.
///
/// #### Debian
///
/// <https://packages.debian.org/sid/amd64/atuin/filelist> is not managed by systemctl and is
/// therefore identical to the curl Install Script case above, governed entirely by
/// `enabled`/`autostart`.
///
/// #### Ubuntu
///
/// <https://packages.ubuntu.com/questing/amd64/atuin/filelist> is the same as the curl Install
/// Script case above.
///
/// #### Fedora
///
/// <https://packages.fedoraproject.org/pkgs/atuin/atuin/> seems identical to the curl Install
/// Script case.
///
/// #### Arch
///
/// <https://archlinux.org/packages/extra/x86_64/atuin/> is identical to the curl Install Script
/// case -- we manage the daemon.
///
/// #### Alpine
///
/// <https://pkgs.alpinelinux.org/package/edge/community/x86_64/atuin> -- the main package install
/// matches the curl Install Script case.
///
/// #### Void
///
/// <https://raw.githubusercontent.com/void-linux/void-packages/master/srcpkgs/atuin/template> --
/// binary-only: the template installs the binary, license, and completions, and the `srcpkgs/atuin`
/// dir contains only `template` (no `patches/`, no `files/`). Void is runit-based with no daemon
/// service, so `systemd_socket` is irrelevant and behavior matches the curl Install Script case.
///
/// #### home-manager (Nix)
///
/// <https://github.com/nix-community/home-manager/blob/master/modules/programs/atuin.nix> exposes
/// `programs.atuin.daemon.enable`. When enabled it generates a `systemd.user.services.atuin-daemon` +
/// `systemd.user.sockets.atuin-daemon` pair on Linux, or a `launchd.agents.atuin-daemon` on
/// macOS. Crucially, it also WRITES our config into `config.toml`:
///
///   - `daemon.enabled = true` (always),
///   - `daemon.systemd_socket = true` on systemd, `false` on launchd,
///   - `daemon.socket_path = $XDG_DATA_HOME/atuin/daemon.sock` on launchd only.
///
/// It never sets `daemon.autostart`, so it stays `false`. That means the client ALWAYS takes the
/// bail-on-mismatch path -- it never sends `Shutdown` and never respawns the daemon itself.
/// Replacing the daemon is left to the service manager plus `home-manager switch`.
///
///   - On Linux (`systemd_socket = true`, `autostart = false`), the `.socket` listens on
///     `%t/atuin.sock` (`$XDG_RUNTIME_DIR/atuin.sock`), exactly our socket-activation path. On a
///     mismatch the client bails. A rebuild changes the unit's `ExecStart` store path, so
///     activation (the default `sd-switch` backend) restarts the unit; the next connection then
///     socket-activates the new binary.
///   - On macOS (`systemd_socket = false`, `autostart = false`), the launchd agent uses a
///     conditional `KeepAlive` (`Crashed = true; SuccessfulExit = false`), so launchd will NOT
///     respawn a cleanly-exited daemon -- a stale-version daemon persists until `home-manager
///     switch` reloads the agent (its `ProgramArguments` store path changes).
///
/// Caveat: these settings only land if home-manager can write `config.toml` -- it is generated with
/// `force = forceOverwriteSettings` (default `false`), and since atuin rewrites its own config, a
/// pre-existing real `config.toml` blocks them unless `forceOverwriteSettings = true`.
///
/// #### Gentoo
///
/// The `app-shells/atuin` ebuild, behind the default-on `daemon` USE flag, installs two systemd
/// USER units via `systemd_douserunit` (it patches nothing and writes no config):
/// <https://github.com/gentoo/gentoo/blob/master/app-shells/atuin/files/atuin-daemon.socket> and
/// <https://github.com/gentoo/gentoo/blob/master/app-shells/atuin/files/atuin-daemon.service>.
///
/// The socket listens on `%t/atuin.sock` (`$XDG_RUNTIME_DIR/atuin.sock`, our socket-activation
/// path) with `RemoveOnStop=true`. The service is pure socket-activation: `Requires=` the socket,
/// `ExecStart=atuin daemon` (the deprecated bare form; we warn in favor of `atuin daemon start`),
/// and it sets no `Restart=`, so systemd defaults to `Restart=no` and never supervise-restarts it.
///
/// Because the ebuild sets no config, the user must set `daemon.systemd_socket = true` (and
/// `enabled = true`) themselves; the intended pairing is `systemd_socket = true` + `autostart =
/// false`. On a mismatch the client therefore bails and defers to systemd (autostart is
/// incompatible with `systemd_socket`).
///
/// **In effect an `emerge` upgrade leaves the stale daemon running until it is stopped; the next
/// client connection then socket-activates a fresh daemon from the new binary.**
///
/// ## Evil Cases
///
/// Let's consider some cases that severely risk breaking wire compatibility.
///
/// ### Mismatched Daemon & Client Versions
///
/// If the daemon and client are not running the same daemon protocol version, then a breakage in
/// wire compatibility means one of three things:
///
/// ### You have used the `reserved` keyword
///
/// Modifying the `.proto` by changing existing fields is **heavily advised against**.
/// <https://protobuf.dev/programming-guides/proto3/#assigning> **heavily** urges you to delete
/// fields and mark the deleted field numbers as reserved.
///
/// Consider the case of an updated client and an old server. The old server, as per
/// <https://protobuf.dev/programming-guides/encoding/#structure>, will **skip** old fields, which
/// means that old fields will receive the **default** value. In most cases, this is `Option::None`.
///
/// ### You have modified (or removed) a field
///
/// If you change the type of the field (or remove a field) **without** changing the field number,
/// then one of two things can happen:
///
///   - Either gRPC recognizes the change and returns an error on the mismatched request, or
///   - gRPC is oblivious to the change you've made, in which case it will misinterpret the data.
///     Hopefully your domain conversion catches it, and if it doesn't, the daemon will get
///     mangled data. See <https://protobuf.dev/programming-guides/encoding/#structure> for more
///     info on when gRPC will misinterpret your data (an example is string -> bytes conversions).
///
/// **This can only happen if the `PROTOCOL_VERSION` the daemon was built with and the
/// `PROTOCOL_VERSION` the client was built with are mismatched**, as that avoids the restart path described above.
///
/// **Note that you should never change the `Shutdown` and `Status` RPCs as they do not have the
/// protocol version guards.** They are **assumed** to be stable and if you want to modify them, you
/// **must** use the `reserved` keyword.
pub const PROTOCOL_VERSION: u32 = 5;

/// Boot the daemon using the new component-based architecture.
///
/// This creates a daemon with the search component, spawns the background sync
/// engine, starts the gRPC server with their services, and runs the event loop.
pub async fn boot(
    settings: Settings,
    store: SqliteStore,
    history_db: HistoryDatabase,
) -> Result<()> {
    // Create the components
    let search_component = SearchComponent::new();
    let blocking_pool = BlockingPool::new(MAX_BLOCKING_WORKERS);

    let output_capture = match settings.output.limits() {
        Some(limits) => {
            OutputCaptureEngine::open(Settings::command_capture_dir(), limits.max_disk_usage).await
        }
        None => OutputCaptureEngine::nop(),
    };

    // Get the gRPC services before moving components into the daemon
    // (The services share state with the components via Arc)
    let search_service = search_component.grpc_service(output_capture.store());
    let search_index = search_component.index();

    // Build the daemon
    let mut daemon = Daemon::builder(settings.clone())
        .store(store)
        .history_db(history_db)
        .component(search_component)
        .build()?;

    let handle = daemon.handle();

    let host_id = Settings::host_id().await?;

    let ai_session_db_path = Settings::effective_data_dir().join("ai_harness_sessions.db");
    let ai_session_db = match AiSessionDatabase::open(&ai_session_db_path).await {
        Ok(db) => Some(db),
        Err(err) => {
            tracing::error!(
                ?err,
                path = ?ai_session_db_path,
                "failed to open the ai-session sidecar; ai-session capture is disabled"
            );
            None
        }
    };
    let ai_session_capture = Arc::new(match &ai_session_db {
        Some(db) => {
            let records = AiSessionStore::builder()
                .store(handle.store().clone())
                .host_id(host_id)
                .key(handle.encryption_key().clone())
                .build();

            // Reproject the sidecar from the synced record store before capture starts. The record
            // store is the source of truth; a sidecar that missed an append (transient error,
            // crash between the two writes, or a lost db file) is repaired here instead of being
            // stranded until — or re-pushed as duplicate records by — file re-capture. append's
            // ON CONFLICT keying makes the replay idempotent.
            let recovered = match records.build(db).await {
                Ok(()) => true,
                Err(err) => {
                    tracing::error!(
                        ?err,
                        "failed to reproject ai-session sidecar; capture and import disabled \
                         until restart"
                    );
                    false
                }
            };

            AiHarnessSessionCapture::open(
                records,
                db.clone(),
                settings.ai.capture_sessions,
                recovered,
                blocking_pool.clone(),
            )
        }
        None => AiHarnessSessionCapture::nop().await,
    });

    let _sync_engine = sync::SyncEngine::spawn(handle.clone(), search_index.clone(), ai_session_db);

    let history_store =
        HistoryStore::new(handle.store().clone(), host_id, handle.encryption_key().clone());
    let journal = Arc::new(HistoryJournal::new(
        handle.caps().clone(),
        history_store,
        handle.history_db().clone(),
        search_index,
        output_capture,
    ));
    let history_service = HistoryServer::new(grpc::HistoryService::new(journal, handle.clone()));
    let ai_session_service = AiSessionServer::new(grpc::AiSessionService::new(ai_session_capture));

    // Start all components first (so gRPC services can work)
    daemon.start_components().await?;

    // Spawn config file watcher to reload settings on changes
    if let Ok(watcher) = global_settings_watcher() {
        let mut settings_rx = watcher.subscribe();
        let watcher_handle = handle.clone();
        tokio::spawn(async move {
            tracing::info!("config file watcher started");
            while settings_rx.changed().await.is_ok() {
                // Use the already-loaded settings from the watcher
                // (avoids parsing the config file twice)
                let new_settings = (*settings_rx.borrow()).clone();
                watcher_handle.apply_settings((*new_settings).clone()).await;
            }
            tracing::debug!("config file watcher stopped");
        });
    } else {
        tracing::warn!(
            "failed to start config file watcher; settings changes will require daemon restart"
        );
    }

    // Spawn signal handler to emit ShutdownRequested on Ctrl+C/SIGTERM
    let signal_handle = handle.clone();
    tokio::spawn(async move {
        shutdown_signal().await;
        tracing::info!("received shutdown signal");
        signal_handle.shutdown();
    });

    // Start the gRPC server in the background
    server::run_grpc_server(
        settings,
        history_service,
        search_service.build(handle.clone()),
        ai_session_service,
        handle,
    )
    .await?;

    // Run the daemon event loop
    daemon.run_event_loop().await?;

    // Stop all components on shutdown
    daemon.stop_components().await;

    tracing::info!("daemon shut down complete");
    Ok(())
}

/// Wait for a shutdown signal (Ctrl+C or SIGTERM).
#[cfg(unix)]
async fn shutdown_signal() {
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .expect("failed to register sigterm handler");
    let mut int = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
        .expect("failed to register sigint handler");

    tokio::select! {
        _ = term.recv() => {},
        _ = int.recv() => {},
    }
}

/// Wait for a shutdown signal (Ctrl+C).
#[cfg(not(unix))]
async fn shutdown_signal() {
    tokio::signal::ctrl_c().await.expect("failed to listen for ctrl+c");
}
