pub mod pb;

use std::pin::Pin;
use std::sync::Arc;

use atuin_client::history::{History, HistoryId};
use atuin_common::time::OffsetDateTimeExt;
use easy_cast::Cast;
use futures::StreamExt;
use time::OffsetDateTime;
use tokio_stream::Stream;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;
use tonic::{Request, Response, Status};
use tracing::{Level, instrument};

use crate::DaemonHandle;
use crate::grpc::history::pb::history_server::History as GrpcService;
use crate::grpc::history::pb::{
    CancelHistoryReply, CancelHistoryRequest, DeleteHistoryReply, DeleteHistoryRequest,
    DeleteHistoryStreamExt, EndHistoryReply, EndHistoryRequest, GetCommandOutputRequest,
    GetCommandOutputResponse, Lagged, RebuildHistoryReply, RebuildHistoryRequest,
    RegisterCommandOutputRequest, RegisterCommandOutputResponse, ShutdownReply, ShutdownRequest,
    StartHistoryReply, StartHistoryRequest, StatusReply, StatusRequest, TailHistoryEvent,
    TailHistoryReply, TailHistoryRequest,
};
use crate::history_journal::HistoryJournal;

/// TL;DR:
///
///   - If you change the proto files, ensure you use the `reserved` proto keyword for modifying
///     anything related to the `Shutdown` and `Status` RPCs.
///   - Go ham on breaking wire compatibility on every other RPC. **Make sure you update
///     `DAEMON_PROTOCOL_VERSION` both in here, and in the client. It is defined in two spots, yes.
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
///   2. Checks that the response version `DAEMON_VERSION` (ie. the Atuin version) matches that of
///      the client and also that `DAEMON_PROTOCOL_VERSION` matches that of the client.
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
///   will gracefully be restarted when `DAEMON_PROTOCOL_VERSION` changes. Wire incompatibility is
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
/// `DAEMON_PROTOCOL_VERSION` mismatch and the daemon will get restarted, one way or another.**
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
/// **This can only happen if the `DAEMON_PROTOCOL_VERSION` on the daemon (this one) and the
/// `DAEMON_PROTOCOL_VERSION` on the client (`crates/atuin/src/command/client/daemon.rs`) are
/// mismatched**, as that avoids the restart path described above.
///
/// **Note that you should never change the `Shutdown` and `Status` RPCs as they do not have the
/// protocol version guards.** They are **assumed** to be stable and if you want to modify them, you
/// **must** use the `reserved` keyword.
const DAEMON_PROTOCOL_VERSION: u32 = 2;

/// The History gRPC service.
///
/// Clients request operations on history via this service.
#[derive(Clone)]
pub struct Service {
    journal: Arc<HistoryJournal>,
    /// TODO(markovejnovic): Revisit whether we need to hold this handle. It exists only to service
    /// the [`GrpcService::shutdown`] request.
    daemon_handle: DaemonHandle,
}

impl Service {
    #[must_use]
    pub fn new(journal: Arc<HistoryJournal>, daemon_handle: DaemonHandle) -> Self {
        Self {
            journal,
            daemon_handle,
        }
    }
}

#[tonic::async_trait]
impl GrpcService for Service {
    type TailHistoryStream = Pin<Box<dyn Stream<Item = Result<TailHistoryReply, Status>> + Send>>;

    #[instrument(skip_all, level = Level::TRACE)]
    async fn start_history(
        &self,
        request: Request<StartHistoryRequest>,
    ) -> Result<Response<StartHistoryReply>, Status> {
        let history: History = request.into_inner().try_into()?;

        let id = self.journal.start_cmd(history);

        Ok(Response::new(StartHistoryReply {
            id: Some(id.into()),
            // TODO(markovejnovic): Pull this from one constant, well-defined spot.
            version: env!("CARGO_PKG_VERSION").to_string(),
            protocol: DAEMON_PROTOCOL_VERSION,
        }))
    }

    #[instrument(skip_all, level = Level::TRACE)]
    async fn end_history(
        &self,
        req: Request<EndHistoryRequest>,
    ) -> Result<Response<EndHistoryReply>, Status> {
        let req = req.into_inner().view()?;

        // The client may omit the duration, in which case we measure it from the command's start
        // timestamp, which the journal tracks for us.
        let duration = match req.duration {
            Some(duration) => duration,
            None => OffsetDateTime::now_utc()
                .saturating_duration_since(self.journal.get(req.history_id)?.timestamp),
        };

        let finished_cmd = self.journal.finish(req.history_id, req.exit_code, duration).await?;

        Ok(Response::new(EndHistoryReply {
            record_id: Some(finished_cmd.history_record_id.into()),
            record_idx: finished_cmd.history_record_idx,
            // TODO(markovejnovic): Pull this from one constant, well-defined spot.
            version: env!("CARGO_PKG_VERSION").to_string(),
            protocol: DAEMON_PROTOCOL_VERSION,
        }))
    }

    #[instrument(skip_all, level = Level::TRACE)]
    async fn cancel_history(
        &self,
        request: Request<CancelHistoryRequest>,
    ) -> Result<Response<CancelHistoryReply>, Status> {
        let id: HistoryId = request.into_inner().try_into()?;

        self.journal.cancel(id).await?;

        Ok(Response::new(CancelHistoryReply {
            // TODO(markovejnovic): Pull this from one constant, well-defined spot.
            version: env!("CARGO_PKG_VERSION").to_string(),
            protocol: DAEMON_PROTOCOL_VERSION,
        }))
    }

    #[instrument(skip_all, level = Level::TRACE)]
    async fn delete_history(
        &self,
        request: Request<tonic::Streaming<DeleteHistoryRequest>>,
    ) -> Result<Response<DeleteHistoryReply>, Status> {
        let ids = request.into_inner().collect_history_ids().await?;

        let search_settings = self.daemon_handle.settings().await.search.clone();
        let deleted = self.journal.delete(ids, &search_settings).await?;

        Ok(Response::new(DeleteHistoryReply {
            deleted: deleted.cast(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            protocol: DAEMON_PROTOCOL_VERSION,
        }))
    }

    #[instrument(skip_all, level = Level::TRACE)]
    async fn rebuild_history(
        &self,
        _request: Request<RebuildHistoryRequest>,
    ) -> Result<Response<RebuildHistoryReply>, Status> {
        let search_settings = self.daemon_handle.settings().await.search.clone();
        self.journal.rebuild(&search_settings).await?;

        Ok(Response::new(RebuildHistoryReply {
            version: env!("CARGO_PKG_VERSION").to_string(),
            protocol: DAEMON_PROTOCOL_VERSION,
        }))
    }

    #[instrument(skip_all, level = Level::TRACE)]
    async fn tail_history(
        &self,
        _request: Request<TailHistoryRequest>,
    ) -> Result<Response<Self::TailHistoryStream>, Status> {
        // Every journal event (started, ended, cancelled) and any lag notice becomes a reply on the
        // tail stream.
        let stream = self.journal.subscribe().map(|event| {
            Ok::<_, Status>(TailHistoryReply {
                event: Some(match event {
                    Ok(event) => event.into(),
                    Err(BroadcastStreamRecvError::Lagged(dropped)) => {
                        TailHistoryEvent::Lagged(Lagged { dropped })
                    }
                }),
            })
        });

        Ok(Response::new(Box::pin(stream)))
    }

    /// Returns the active status of the daemon. Has nothing to do with history.
    ///
    /// TODO(markovejnovic): This probably doesn't belong in this service.
    #[instrument(skip_all, level = Level::TRACE)]
    async fn status(
        &self,
        _request: Request<StatusRequest>,
    ) -> Result<Response<StatusReply>, Status> {
        Ok(Response::new(StatusReply {
            healthy: true,
            // TODO(markovejnovic): Pull this from one constant, well-defined spot.
            version: env!("CARGO_PKG_VERSION").to_string(),
            pid: std::process::id(),
            protocol: DAEMON_PROTOCOL_VERSION,
        }))
    }

    /// Requests the daemon shut down. Has nothing to do with history.
    ///
    /// Note:
    ///  - A misbehaving daemon will likely not respect this request.
    ///  - The shutdown request is sent asynchronously, but this RPC immediately returns.
    ///
    /// TODO(markovejnovic): This probably doesn't belong in this service.
    #[instrument(skip_all, level = Level::TRACE)]
    async fn shutdown(
        &self,
        _request: Request<ShutdownRequest>,
    ) -> Result<Response<ShutdownReply>, Status> {
        self.daemon_handle.shutdown();
        Ok(Response::new(ShutdownReply { accepted: true }))
    }

    #[instrument(skip_all, level = Level::TRACE)]
    async fn register_command_output(
        &self,
        request: Request<RegisterCommandOutputRequest>,
    ) -> Result<Response<RegisterCommandOutputResponse>, Status> {
        let request = request.into_inner();
        self.journal
            .register_command_output(request.history_id()?, request.capture()?.into())
            .await?;
        Ok(Response::new(RegisterCommandOutputResponse {}))
    }

    #[instrument(skip_all, level = Level::TRACE)]
    async fn get_command_output(
        &self,
        request: Request<GetCommandOutputRequest>,
    ) -> Result<Response<GetCommandOutputResponse>, Status> {
        let request = request.into_inner();
        let id = request.history_id()?;

        let capture =
            self.journal.get_command_output(id).await?.ok_or_else(|| {
                Status::not_found(format!("no captured output for history id {id}"))
            })?;

        Ok(Response::new(GetCommandOutputResponse::build(capture.into(), request.output_ranges())))
    }
}
