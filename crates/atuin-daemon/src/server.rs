use eyre::WrapErr;

use atuin_client::encryption;
use atuin_client::history::store::HistoryStore;
use atuin_client::record::sqlite_store::SqliteStore;
use atuin_client::settings::Settings;
use std::io::ErrorKind;
#[cfg(unix)]
use std::path::PathBuf;
use std::sync::Arc;
use time::OffsetDateTime;
use tokio::sync::watch;
use tracing::{Level, instrument};

use atuin_client::database::{Database, Sqlite as HistoryDatabase};
use atuin_client::history::{History, HistoryId};
use dashmap::DashMap;
use eyre::Result;
use tonic::{Request, Response, Status, transport::Server};

use crate::history::history_server::{History as HistorySvc, HistoryServer};

use crate::history::{EndHistoryReply, EndHistoryRequest, StartHistoryReply, StartHistoryRequest};
use crate::history::{ShutdownReply, ShutdownRequest, StatusReply, StatusRequest};

mod sync;

const DAEMON_PROTOCOL_VERSION: u32 = 1;

#[derive(Debug)]
pub struct HistoryService {
    // A store for WIP history
    // This is history that has not yet been completed, aka a command that's current running.
    running: Arc<DashMap<HistoryId, History>>,
    store: HistoryStore,
    history_db: HistoryDatabase,
    shutdown_tx: watch::Sender<bool>,
}

impl HistoryService {
    pub fn new(
        store: HistoryStore,
        history_db: HistoryDatabase,
        shutdown_tx: watch::Sender<bool>,
    ) -> Self {
        Self {
            running: Arc::new(DashMap::new()),
            store,
            history_db,
            shutdown_tx,
        }
    }
}

#[tonic::async_trait()]
impl HistorySvc for HistoryService {
    #[instrument(skip_all, level = Level::INFO)]
    async fn start_history(
        &self,
        request: Request<StartHistoryRequest>,
    ) -> Result<Response<StartHistoryReply>, Status> {
        let running = self.running.clone();
        let req = request.into_inner();

        let timestamp =
            OffsetDateTime::from_unix_timestamp_nanos(req.timestamp as i128).map_err(|_| {
                Status::invalid_argument(
                    "failed to parse timestamp as unix time (expected nanos since epoch)",
                )
            })?;

        let mut h: History = History::daemon()
            .timestamp(timestamp)
            .command(req.command)
            .cwd(req.cwd)
            .session(req.session)
            .hostname(req.hostname)
            .build()
            .into();
        if !req.author.trim().is_empty() {
            h.author = req.author;
        }
        if !req.intent.trim().is_empty() {
            h.intent = Some(req.intent);
        }

        // The old behaviour had us inserting half-finished history records into the database
        // The new behaviour no longer allows that.
        // History that's running is stored in-memory by the daemon, and only committed when
        // complete.
        // If anyone relied on the old behaviour, we could perhaps insert to the history db here
        // too. I'd rather keep it pure, unless that ends up being the case.
        let id = h.id.clone();
        tracing::info!(id = id.to_string(), "start history");
        running.insert(id.clone(), h);

        let reply = StartHistoryReply {
            id: id.to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            protocol: DAEMON_PROTOCOL_VERSION,
        };

        Ok(Response::new(reply))
    }

    #[instrument(skip_all, level = Level::INFO)]
    async fn end_history(
        &self,
        request: Request<EndHistoryRequest>,
    ) -> Result<Response<EndHistoryReply>, Status> {
        let running = self.running.clone();
        let req = request.into_inner();

        let id = HistoryId(req.id);

        if let Some((_, mut history)) = running.remove(&id) {
            history.exit = req.exit;
            history.duration = match req.duration {
                0 => i64::try_from(
                    (OffsetDateTime::now_utc() - history.timestamp).whole_nanoseconds(),
                )
                .expect("failed to convert calculated duration to i64"),
                value => i64::try_from(value).expect("failed to get i64 duration"),
            };

            // Perhaps allow the incremental build to handle this entirely.
            self.history_db
                .save(&history)
                .await
                .map_err(|e| Status::internal(format!("failed to write to db: {e:?}")))?;

            tracing::info!(
                id = id.0.to_string(),
                duration = history.duration,
                "end history"
            );

            let (id, idx) =
                self.store.push(history).await.map_err(|e| {
                    Status::internal(format!("failed to push record to store: {e:?}"))
                })?;

            let reply = EndHistoryReply {
                id: id.0.to_string(),
                idx,
                version: env!("CARGO_PKG_VERSION").to_string(),
                protocol: DAEMON_PROTOCOL_VERSION,
            };

            return Ok(Response::new(reply));
        }

        Err(Status::not_found(format!(
            "could not find history with id: {id}"
        )))
    }

    #[instrument(skip_all, level = Level::INFO)]
    async fn status(
        &self,
        _request: Request<StatusRequest>,
    ) -> Result<Response<StatusReply>, Status> {
        let reply = StatusReply {
            // If status RPC responds, the daemon control plane is healthy.
            healthy: true,
            version: env!("CARGO_PKG_VERSION").to_string(),
            pid: std::process::id(),
            protocol: DAEMON_PROTOCOL_VERSION,
        };

        Ok(Response::new(reply))
    }

    #[instrument(skip_all, level = Level::INFO)]
    async fn shutdown(
        &self,
        _request: Request<ShutdownRequest>,
    ) -> Result<Response<ShutdownReply>, Status> {
        let _ = self.shutdown_tx.send(true);
        Ok(Response::new(ShutdownReply { accepted: true }))
    }
}

#[cfg(unix)]
async fn shutdown_signal(socket: Option<PathBuf>, mut shutdown_rx: watch::Receiver<bool>) {
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .expect("failed to register sigterm handler");
    let mut int = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
        .expect("failed to register sigint handler");

    tokio::select! {
        _  = term.recv() => {},
        _  = int.recv() => {},
        _  = shutdown_rx.changed() => {},
    }

    eprintln!("Removing socket...");
    if let Some(socket) = socket {
        match std::fs::remove_file(socket) {
            Ok(()) => {}
            Err(err) if err.kind() == ErrorKind::NotFound => {}
            Err(err) => {
                eprintln!("failed to remove socket: {err}");
            }
        }
    }
    eprintln!("Shutting down...");
}

#[cfg(windows)]
async fn shutdown_signal(mut shutdown_rx: watch::Receiver<bool>) {
    let mut ctrl_c = tokio::signal::windows::ctrl_c().expect("failed to register signal handler");
    tokio::select! {
        _ = ctrl_c.recv() => {},
        _ = shutdown_rx.changed() => {},
    }
    eprintln!("Shutting down...");
}

#[cfg(unix)]
async fn start_server(
    settings: Settings,
    history: HistoryService,
    shutdown_rx: watch::Receiver<bool>,
) -> Result<()> {
    use tokio::net::UnixListener;
    use tokio_stream::wrappers::UnixListenerStream;

    let socket_path = settings.daemon.socket_path;

    let (uds, cleanup) = if cfg!(target_os = "linux") && settings.daemon.systemd_socket {
        #[cfg(target_os = "linux")]
        {
            use eyre::OptionExt;
            tracing::info!("getting systemd socket");
            let listener = listenfd::ListenFd::from_env()
                .take_unix_listener(0)?
                .ok_or_eyre("missing systemd socket")?;
            listener.set_nonblocking(true)?;
            let actual_path = listener
                .local_addr()
                .context("getting systemd socket's path")
                .and_then(|addr| {
                    addr.as_pathname()
                        .ok_or_eyre("systemd socket missing path")
                        .map(|path| path.to_owned())
                });
            match actual_path {
                Ok(actual_path) => {
                    tracing::info!("listening on systemd socket: {actual_path:?}");
                    if actual_path != std::path::Path::new(&socket_path) {
                        tracing::warn!(
                            "systemd socket is not at configured client path: {socket_path:?}"
                        );
                    }
                }
                Err(err) => {
                    tracing::warn!(
                        "could not detect systemd socket path, ensure that it's at the configured path: {socket_path:?}, error: {err:?}"
                    );
                }
            }
            (UnixListener::from_std(listener)?, false)
        }
        #[cfg(not(target_os = "linux"))]
        unreachable!()
    } else {
        tracing::info!("listening on unix socket {socket_path:?}");
        (UnixListener::bind(socket_path.clone())?, true)
    };

    let uds_stream = UnixListenerStream::new(uds);

    Server::builder()
        .add_service(HistoryServer::new(history))
        .serve_with_incoming_shutdown(
            uds_stream,
            shutdown_signal(cleanup.then_some(socket_path.into()), shutdown_rx),
        )
        .await?;

    Ok(())
}

#[cfg(not(unix))]
async fn start_server(
    settings: Settings,
    history: HistoryService,
    shutdown_rx: watch::Receiver<bool>,
) -> Result<()> {
    use tokio::net::TcpListener;
    use tokio_stream::wrappers::TcpListenerStream;

    let port = settings.daemon.tcp_port;
    let url = format!("127.0.0.1:{port}");
    let tcp = TcpListener::bind(url).await?;
    let tcp_stream = TcpListenerStream::new(tcp);

    tracing::info!("listening on tcp port {:?}", port);

    Server::builder()
        .add_service(HistoryServer::new(history))
        .serve_with_incoming_shutdown(tcp_stream, shutdown_signal(shutdown_rx))
        .await?;
    Ok(())
}

// break the above down when we end up with multiple services

/// Listen on a unix socket
/// Pass the path to the socket
pub async fn listen(
    settings: Settings,
    store: SqliteStore,
    history_db: HistoryDatabase,
) -> Result<()> {
    let encryption_key: [u8; 32] = encryption::load_key(&settings)
        .context("could not load encryption key")?
        .into();

    let host_id = Settings::host_id().await?;
    let history_store = HistoryStore::new(store.clone(), host_id, encryption_key);

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let history = HistoryService::new(history_store.clone(), history_db.clone(), shutdown_tx);

    // start services
    tokio::spawn(sync::worker(
        settings.clone(),
        store,
        history_store,
        history_db,
    ));

    start_server(settings, history, shutdown_rx).await
}
