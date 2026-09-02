pub mod pb;

use std::pin::Pin;
use std::sync::Arc;

use atuin_client::history::{History, HistoryId};
use atuin_common::time::OffsetDateTimeExt;
use futures::StreamExt;
use time::OffsetDateTime;
use tokio_stream::Stream;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;
use tonic::{Request, Response, Status};
use tracing::{Level, instrument};

use crate::DaemonHandle;
use crate::grpc::history::pb::history_server::History as GrpcService;
use crate::grpc::history::pb::{
    CancelHistoryReply, CancelHistoryRequest, EndHistoryReply, EndHistoryRequest, Lagged,
    ShutdownReply, ShutdownRequest, StartHistoryReply, StartHistoryRequest, StatusReply,
    StatusRequest, TailHistoryEvent, TailHistoryReply, TailHistoryRequest,
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
/// client. Each client->daemon connection goes through this function, which:
///
///   1. Probes the daemon via the `history.Status` RPC.
///   2. Checks that the response version `DAEMON_VERSION` (ie. the Atuin version) matches that of
///      the client and also that `DAEMON_PROTOCOL_VERSION` matches that of the client.
///   3. If there is a mismatch, one of two things will happen:
///     a) If `daemon.autostart = false`, then we kindly ask the user to restart the daemon and
///        return an error.
///     b) If `daemon.autostart = true`, then we try to restart the daemon. This means one of
///        multiple things:
///
///        - If `daemon.systemd_socket` is set to some value, we tell the user to restart the
///          daemon, and we bail out.
///        - Otherwise, we send a `history.Shutdown` RPC (version unchecked) to the daemon which
///          causes the daemon to restart. We then spinloop until the daemon is back online.
///
///   4. At this point, either:
///     - We were unable to restart the daemon and have told the user to manually restart or
///     - We have restarted the daemon.
///
/// ### Transport Layer
///
/// I would like to note that restarting the daemon causes the UNIX socket file to be completely
/// erased, effectively flushing any messages that are enqueued to the daemon.
///
/// One of the concerns was that the daemon would receive old data, even if both the client **and**
/// the daemon were to be restarted, since stale data could be queued. Luckily, that's not the case!
///
/// ### Different Package Managers
///
/// There are many ways to install Atuin and consequently let's analyze which installation method
/// hits which code path.
///
/// #### curl Install Script
///
/// - If `daemon.autostart = true` (default), the daemon will gracefully be restarted when
///   `DAEMON_PROTOCOL_VERSION` changes. Wire incompatibility is a-ok in this case.
/// - If `daemon.autostart = false`, we will tell the user to restart Atuin.
///
/// #### Homebrew
///
/// <https://raw.githubusercontent.com/Homebrew/homebrew-core/master/Formula/a/atuin.rb> presents
/// the user with a `homebrew service`. This means the user can run `homebrew service start` to run
/// a new atuin daemon.
///
/// We will try to restart it via the `Shutdown` RPC. When the daemon attempts to game end, the
/// `keep_alive` flag in the homebrew service will restart it.
///
/// This means that, by default, we'll get a new version up and running after an update.
///
/// #### home-manager
///
/// <https://github.com/nix-community/home-manager/blob/master/modules/programs/atuin.nix>
///
/// #### Gentoo
///
/// <https://github.com/gentoo/gentoo/blob/master/app-shells/atuin/files/atuin-daemon.service>
///
/// #### Debian
///
/// <https://packages.debian.org/sid/amd64/atuin/filelist>
///
/// #### Ubuntu
///
/// <https://packages.ubuntu.com/questing/amd64/atuin/filelist>
///
/// #### Fedora
///
/// <https://packages.fedoraproject.org/pkgs/atuin/atuin/>
///
/// #### Arch
///
/// <https://archlinux.org/packages/extra/x86_64/atuin/files/>
///
/// #### Alpine
///
/// <https://pkgs.alpinelinux.org/package/edge/community/x86_64/atuin>
///
/// #### Void
///
/// <https://raw.githubusercontent.com/void-linux/void-packages/master/srcpkgs/atuin/template>
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
///
/// ## Conclusion
const DAEMON_PROTOCOL_VERSION: u32 = 2;

/// The History gRPC service.
///
/// Clients request operations on history via this service.
#[derive(Clone)]
pub struct Service {
    journal: Arc<HistoryJournal>,
    /// TODO(markovejnovic): Revisit whether we need to hold this handle. At the moment, the only
    /// reason why this exists is to be able to service the [`GrpcService::shutdown`] request, but
    /// perhaps that function does not belong in the history service -- perhaps we should have a
    /// Control service.
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

        self.journal.cancel(id)?;

        Ok(Response::new(CancelHistoryReply {
            // TODO(markovejnovic): Pull this from one constant, well-defined spot.
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
}
