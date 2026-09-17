//! Head-to-head: the daemon history flow OLD (up-front Status probe, then the real RPC) vs NEW
//! (send the real RPC optimistically, validate its reply). Both talk to the same in-process mock
//! History gRPC server over a unix socket, so the comparison isolates the round-trip the probe adds
//! rather than any server-side cost.
//!
//! The `atuin` binary crate has no lib target, so the client flow logic lives here in the bench,
//! replicating what `command::client::daemon::try_with_restart` does on the happy path. An atomic
//! counter on the server proves the structural win (RPC round-trips per flow), printed before the
//! divan wall-time run.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

use atuin_daemon::grpc::history::pb;
use atuin_daemon::grpc::history::pb::history_client::HistoryClient;
use atuin_daemon::grpc::history::pb::history_server::{History, HistoryServer};
use divan::black_box;
use hyper_util::rt::TokioIo;
use tokio::net::{UnixListener, UnixStream};
use tokio::runtime::Runtime;
use tokio_stream::Stream;
use tokio_stream::wrappers::UnixListenerStream;
use tonic::transport::{Channel, Endpoint, Server, Uri};
use tonic::{Request, Response, Status};
use tower::service_fn;

const VERSION: &str = env!("CARGO_PKG_VERSION");
const PROTOCOL: u32 = 2;

/// Minimal History service: it answers the three RPCs the flow touches (`status`, `start_history`,
/// `end_history`) with a version/protocol the client accepts, and counts every served RPC.
#[derive(Clone)]
struct MockHistory {
    rpcs: Arc<AtomicU64>,
}

#[tonic::async_trait]
impl History for MockHistory {
    type TailHistoryStream =
        Pin<Box<dyn Stream<Item = Result<pb::TailHistoryReply, Status>> + Send>>;

    async fn start_history(
        &self,
        _req: Request<pb::StartHistoryRequest>,
    ) -> Result<Response<pb::StartHistoryReply>, Status> {
        self.rpcs.fetch_add(1, Ordering::Relaxed);
        Ok(Response::new(pb::StartHistoryReply {
            id: None,
            version: VERSION.to_string(),
            protocol: PROTOCOL,
        }))
    }

    async fn end_history(
        &self,
        _req: Request<pb::EndHistoryRequest>,
    ) -> Result<Response<pb::EndHistoryReply>, Status> {
        self.rpcs.fetch_add(1, Ordering::Relaxed);
        Ok(Response::new(pb::EndHistoryReply {
            record_id: None,
            record_idx: 0,
            version: VERSION.to_string(),
            protocol: PROTOCOL,
        }))
    }

    async fn status(
        &self,
        _req: Request<pb::StatusRequest>,
    ) -> Result<Response<pb::StatusReply>, Status> {
        self.rpcs.fetch_add(1, Ordering::Relaxed);
        Ok(Response::new(pb::StatusReply {
            healthy: true,
            version: VERSION.to_string(),
            pid: std::process::id(),
            protocol: PROTOCOL,
        }))
    }

    async fn cancel_history(
        &self,
        _req: Request<pb::CancelHistoryRequest>,
    ) -> Result<Response<pb::CancelHistoryReply>, Status> {
        Err(Status::unimplemented("bench mock"))
    }

    async fn delete_history(
        &self,
        _req: Request<tonic::Streaming<pb::DeleteHistoryRequest>>,
    ) -> Result<Response<pb::DeleteHistoryReply>, Status> {
        Err(Status::unimplemented("bench mock"))
    }

    async fn rebuild_history(
        &self,
        _req: Request<pb::RebuildHistoryRequest>,
    ) -> Result<Response<pb::RebuildHistoryReply>, Status> {
        Err(Status::unimplemented("bench mock"))
    }

    async fn tail_history(
        &self,
        _req: Request<pb::TailHistoryRequest>,
    ) -> Result<Response<Self::TailHistoryStream>, Status> {
        Err(Status::unimplemented("bench mock"))
    }

    async fn shutdown(
        &self,
        _req: Request<pb::ShutdownRequest>,
    ) -> Result<Response<pb::ShutdownReply>, Status> {
        Err(Status::unimplemented("bench mock"))
    }

    async fn register_command_output(
        &self,
        _req: Request<pb::RegisterCommandOutputRequest>,
    ) -> Result<Response<pb::RegisterCommandOutputResponse>, Status> {
        Err(Status::unimplemented("bench mock"))
    }

    async fn get_command_output(
        &self,
        _req: Request<pb::GetCommandOutputRequest>,
    ) -> Result<Response<pb::GetCommandOutputResponse>, Status> {
        Err(Status::unimplemented("bench mock"))
    }
}

struct Harness {
    rt: Runtime,
    socket: PathBuf,
    rpcs: Arc<AtomicU64>,
    _tmp: tempfile::TempDir,
}

fn harness() -> &'static Harness {
    static HARNESS: OnceLock<Harness> = OnceLock::new();
    HARNESS.get_or_init(|| {
        let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let socket = tmp.path().join("bench.sock");
        let rpcs = Arc::new(AtomicU64::new(0));

        let service = HistoryServer::new(MockHistory { rpcs: rpcs.clone() });
        let listen_socket = socket.clone();
        rt.spawn(async move {
            let uds = UnixListener::bind(&listen_socket).unwrap();
            Server::builder()
                .add_service(service)
                .serve_with_incoming(UnixListenerStream::new(uds))
                .await
                .unwrap();
        });

        // Wait for the server to start accepting.
        rt.block_on(async {
            for _ in 0..500 {
                if connect(&socket).await.is_ok() {
                    return;
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
            panic!("mock daemon never came up");
        });

        Harness {
            rt,
            socket,
            rpcs,
            _tmp: tmp,
        }
    })
}

/// A fresh connection, exactly like every shell hook makes (`HistoryClient::new`).
async fn connect(socket: &Path) -> Result<HistoryClient<Channel>, tonic::transport::Error> {
    let path = socket.to_path_buf();
    let channel = Endpoint::try_from("http://atuin_local_daemon:0")
        .unwrap()
        .connect_with_connector(service_fn(move |_: Uri| {
            let path = path.clone();
            async move { Ok::<_, std::io::Error>(TokioIo::new(UnixStream::connect(path).await?)) }
        }))
        .await?;
    Ok(HistoryClient::new(channel))
}

fn start_request() -> pb::StartHistoryRequest {
    pb::StartHistoryRequest {
        timestamp: 1_700_000_000_000_000_000,
        command: "git status --short".to_string(),
        cwd: "/home/user/project".to_string(),
        session: "0192f0a0-0000-7000-8000-000000000000".to_string(),
        hostname: "host:user".to_string(),
        author: "user".to_string(),
        intent: String::new(),
        shell: "bash".to_string(),
        author_kind: 0,
    }
}

fn end_request() -> pb::EndHistoryRequest {
    pb::EndHistoryRequest {
        id: None,
        exit: 0,
        duration: None,
    }
}

// OLD flow: probe (Status) then the real RPC, on a fresh connection per operation.
async fn old_start(socket: &Path) {
    let mut client = connect(socket).await.unwrap();
    black_box(client.status(pb::StatusRequest {}).await.unwrap());
    black_box(client.start_history(start_request()).await.unwrap());
}

async fn old_end(socket: &Path) {
    let mut client = connect(socket).await.unwrap();
    black_box(client.status(pb::StatusRequest {}).await.unwrap());
    black_box(client.end_history(end_request()).await.unwrap());
}

// NEW flow: send the real RPC optimistically, no probe.
async fn new_start(socket: &Path) {
    let mut client = connect(socket).await.unwrap();
    black_box(client.start_history(start_request()).await.unwrap());
}

async fn new_end(socket: &Path) {
    let mut client = connect(socket).await.unwrap();
    black_box(client.end_history(end_request()).await.unwrap());
}

/// A full command lifecycle: `start_history` followed by `end_history`, each on its own fresh
/// connection, as the shell hooks do.
#[divan::bench(min_time = 1)]
fn lifecycle_old(bencher: divan::Bencher) {
    let h = harness();
    bencher.bench(|| {
        h.rt.block_on(async {
            old_start(&h.socket).await;
            old_end(&h.socket).await;
        });
    });
}

#[divan::bench(min_time = 1)]
fn lifecycle_new(bencher: divan::Bencher) {
    let h = harness();
    bencher.bench(|| {
        h.rt.block_on(async {
            new_start(&h.socket).await;
            new_end(&h.socket).await;
        });
    });
}

/// The same lifecycle with the connection amortized (built untimed), so the timed region is just
/// the RPC round-trips: this isolates the round-trip the probe adds.
#[divan::bench(min_time = 1)]
fn rpcs_old(bencher: divan::Bencher) {
    let h = harness();
    bencher.with_inputs(|| h.rt.block_on(connect(&h.socket)).unwrap()).bench_values(
        |mut client| {
            h.rt.block_on(async {
                black_box(client.status(pb::StatusRequest {}).await.unwrap());
                black_box(client.start_history(start_request()).await.unwrap());
                black_box(client.status(pb::StatusRequest {}).await.unwrap());
                black_box(client.end_history(end_request()).await.unwrap());
            });
        },
    );
}

#[divan::bench(min_time = 1)]
fn rpcs_new(bencher: divan::Bencher) {
    let h = harness();
    bencher.with_inputs(|| h.rt.block_on(connect(&h.socket)).unwrap()).bench_values(
        |mut client| {
            h.rt.block_on(async {
                black_box(client.start_history(start_request()).await.unwrap());
                black_box(client.end_history(end_request()).await.unwrap());
            });
        },
    );
}

fn main() {
    let h = harness();

    // Structural proof: RPC round-trips per command lifecycle. This is the headline win; the
    // wall-time benches below merely confirm it is not lost in the noise.
    let before = h.rpcs.load(Ordering::Relaxed);
    h.rt.block_on(async {
        old_start(&h.socket).await;
        old_end(&h.socket).await;
    });
    let old_rpcs = h.rpcs.load(Ordering::Relaxed) - before;

    let before = h.rpcs.load(Ordering::Relaxed);
    h.rt.block_on(async {
        new_start(&h.socket).await;
        new_end(&h.socket).await;
    });
    let new_rpcs = h.rpcs.load(Ordering::Relaxed) - before;

    eprintln!(
        "RPC round-trips per command lifecycle (start+end): OLD = {old_rpcs}, NEW = {new_rpcs}"
    );
    assert_eq!(old_rpcs, 4, "old flow should issue 2 Status probes + 2 real RPCs");
    assert_eq!(new_rpcs, 2, "new flow should issue only the 2 real RPCs");

    divan::main();
}
