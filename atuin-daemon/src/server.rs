use atuin_client::history::store::HistoryStore;
use atuin_client::record::sqlite_store::SqliteStore;
use std::path::PathBuf;
use std::sync::Arc;
use time::OffsetDateTime;

use atuin_client::database::{Database, Sqlite as HistoryDatabase};
use atuin_client::history::{History, HistoryId};
use dashmap::DashMap;
use eyre::Result;
use tokio::net::UnixListener;
use tokio_stream::wrappers::UnixListenerStream;
use tonic::{transport::Server, Request, Response, Status};

use crate::history::history_server::{History as HistorySvc, HistoryServer};

use crate::history::{EndHistoryReply, EndHistoryRequest, StartHistoryReply, StartHistoryRequest};

pub struct HistoryService {
    // A store for WIP history
    // This is history that has not yet been completed, aka a command that's current running.
    running: Arc<DashMap<HistoryId, History>>,
    store: HistoryStore,
    history_db: HistoryDatabase,
}

impl HistoryService {
    pub fn new(store: HistoryStore, history_db: HistoryDatabase) -> Self {
        Self {
            running: Arc::new(DashMap::new()),
            store,
            history_db,
        }
    }
}

#[tonic::async_trait()]
impl HistorySvc for HistoryService {
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

        let h: History = History::daemon()
            .timestamp(timestamp)
            .command(req.command)
            .cwd(req.cwd)
            .session(req.session)
            .hostname(req.hostname)
            .build()
            .into();

        println!("request to start {} {}", h.id, h.command);

        // The old behaviour had us inserting half-finished history records into the database
        // The new behaviour no longer allows that.
        // History that's running is stored in-memory by the daemon, and only committed when
        // complete.
        // If anyone relied on the old behaviour, we could perhaps insert to the history db here
        // too. I'd rather keep it pure, unless that ends up being the case.
        let id = h.id.clone();
        running.insert(id.clone(), h);

        let reply = StartHistoryReply { id: id.to_string() };

        Ok(Response::new(reply))
    }

    async fn end_history(
        &self,
        request: Request<EndHistoryRequest>,
    ) -> Result<Response<EndHistoryReply>, Status> {
        let running = self.running.clone();
        let req = request.into_inner();
        println!("Got end history request {}", req.id);

        let id = HistoryId(req.id);

        if let Some((_, mut history)) = running.remove(&id) {
            println!("request to end {}", history.command);

            history.exit = req.exit;
            history.duration = match req.duration {
                Some(value) => i64::try_from(value).expect("failed to get i64 duration"),
                None => i64::try_from(
                    (OffsetDateTime::now_utc() - history.timestamp).whole_nanoseconds(),
                )
                .expect("failed to convert calculated duration to i64"),
            };

            // Perhaps allow the incremental build to handle this entirely.
            self.history_db
                .save(&history)
                .await
                .map_err(|e| Status::internal(format!("failed to write to db: {e:?}")))?;

            let (id, idx) =
                self.store.push(history).await.map_err(|e| {
                    Status::internal(format!("failed to push record to store: {e:?}"))
                })?;

            let reply = EndHistoryReply {
                id: id.0.to_string(),
                idx,
            };

            return Ok(Response::new(reply));
        }

        println!("Failed to find history with id: {id:?}, running: {running:?}");

        Err(Status::not_found(format!(
            "could not find history with id: {id}"
        )))
    }
}

// break the above down when we end up with multiple services

/// Listen on a unix socket
/// Pass the path to the socket
pub async fn listen(
    store: HistoryStore,
    history_db: HistoryDatabase,
    socket: PathBuf,
) -> Result<()> {
    let history = HistoryService::new(store, history_db);

    let uds = UnixListener::bind(socket.clone())?;
    let uds_stream = UnixListenerStream::new(uds);

    Server::builder()
        .add_service(HistoryServer::new(history))
        .serve_with_incoming(uds_stream)
        .await?;

    std::fs::remove_file(socket)?;

    Ok(())
}
