pub mod pb;

use std::pin::Pin;
use std::process::Stdio;
use std::sync::Arc;

use atuin_client::history::{History, HistoryId};
use atuin_common::time::OffsetDateTimeExt;
use easy_cast::Cast;
use futures::StreamExt;
use time::OffsetDateTime;
use tokio_stream::Stream;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;
use tonic::{Request, Response, Status};
use tracing::{Instrument, Level, instrument};

// `Lagged` now lives in the shared `common` package (see `common.proto`).
use crate::grpc::common::pb::Lagged;
use crate::grpc::history::pb::history_server::History as GrpcService;
use crate::grpc::history::pb::{
    CancelHistoryReply, CancelHistoryRequest, CompactStoreReply, CompactStoreRequest,
    DeleteHistoryReply, DeleteHistoryRequest, DeleteHistoryStreamExt, EndHistoryReply,
    EndHistoryRequest, GetCommandOutputRequest, GetCommandOutputResponse, ImportHistoryReply,
    ImportHistoryRequest, LoggedInReply, LoggedInRequest, RebuildHistoryReply,
    RebuildHistoryRequest, RegisterCommandOutputRequest, RegisterCommandOutputResponse,
    ShutdownReply, ShutdownRequest, StartHistoryReply, StartHistoryRequest, StatusReply,
    StatusRequest, TailHistoryEvent, TailHistoryReply, TailHistoryRequest,
};
use crate::history_journal::HistoryJournal;
use crate::{DaemonEvent, DaemonHandle};

/// The History gRPC service.
///
/// Clients request operations on history via this service.
#[derive(Clone)]
pub struct Service {
    journal: Arc<HistoryJournal>,
    /// TODO(markovejnovic): Revisit whether we need to hold this handle. It exists only to service
    /// the [`GrpcService::shutdown`] and [`GrpcService::logged_in`] requests.
    daemon_handle: DaemonHandle,
}

/// Errors returned by [`Service::import_shell_history`].
#[derive(Debug, thiserror::Error)]
enum ImportHistoryError {
    #[error("couldn't run `atuin import {shell}`: {source}")]
    Spawn {
        shell: String,
        #[source]
        source: std::io::Error,
    },
    #[error("`atuin import {shell}` failed: {stderr}")]
    Failed {
        shell: String,
        stderr: String,
    },
}

impl Service {
    #[must_use]
    pub fn new(journal: Arc<HistoryJournal>, daemon_handle: DaemonHandle) -> Self {
        Self {
            journal,
            daemon_handle,
        }
    }

    /// Import `shell`'s history with the CLI at `exe`, then have search and sync see it.
    async fn import_shell_history(
        &self,
        exe: &std::path::Path,
        shell: &str,
        histfile: Option<String>,
        fish_history: Option<String>,
    ) -> Result<(), ImportHistoryError> {
        // The importers are the CLI's.
        let mut command = tokio::process::Command::new(exe);
        command.args(["import", shell]).stdin(Stdio::null()).stdout(Stdio::null());
        // Where the client's history is, not where the daemon's own environment says, which may
        // be another shell's.
        for (name, value) in [("HISTFILE", &histfile), ("fish_history", &fish_history)] {
            match value {
                Some(value) => command.env(name, value),
                None => command.env_remove(name),
            };
        }
        let output = command.output().await.map_err(|source| ImportHistoryError::Spawn {
            shell: shell.to_owned(),
            source,
        })?;
        if !output.status.success() {
            return Err(ImportHistoryError::Failed {
                shell: shell.to_owned(),
                stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
            });
        }

        // Search reads its index, which doesn't see rows written by another process.
        let search = self.daemon_handle.settings().await.search.clone();
        self.journal.reload_search_index(&search).await;
        tracing::info!(shell, "imported shell history");
        self.daemon_handle.request_sync();
        Ok(())
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

        let journal = self.journal.clone();
        let (history_id, exit_code) = (req.history_id, req.exit_code);
        // Spawned so a client disconnect cannot drop the call half-way. Otherwise the entry could
        // be saved to the history db but never added to the search index.
        let finished_cmd = tokio::spawn(
            async move { journal.finish(history_id, exit_code, duration).await }
                .instrument(tracing::Span::current()),
        )
        .await
        .map_err(|e| Status::internal(format!("finish did not complete: {e}")))??;

        Ok(Response::new(EndHistoryReply {
            record_id: Some(finished_cmd.history_record_id.into()),
            record_idx: finished_cmd.history_record_idx,
        }))
    }

    #[instrument(skip_all, level = Level::TRACE)]
    async fn cancel_history(
        &self,
        request: Request<CancelHistoryRequest>,
    ) -> Result<Response<CancelHistoryReply>, Status> {
        let id: HistoryId = request.into_inner().try_into()?;

        let journal = self.journal.clone();
        // Spawned so a client disconnect cannot drop the call half-way.
        tokio::spawn(async move { journal.cancel(id).await }.instrument(tracing::Span::current()))
            .await
            .map_err(|e| Status::internal(format!("cancel did not complete: {e}")))??;

        Ok(Response::new(CancelHistoryReply {}))
    }

    #[instrument(skip_all, level = Level::TRACE)]
    async fn delete_history(
        &self,
        request: Request<tonic::Streaming<DeleteHistoryRequest>>,
    ) -> Result<Response<DeleteHistoryReply>, Status> {
        let ids = request.into_inner().collect_history_ids().await?;

        let search_settings = self.daemon_handle.settings().await.search.clone();
        let journal = self.journal.clone();
        // Spawned so a client disconnect cannot drop the call half-way.
        let deleted = tokio::spawn(
            async move { journal.delete(&ids, &search_settings).await }
                .instrument(tracing::Span::current()),
        )
        .await
        .map_err(|e| Status::internal(format!("delete did not complete: {e}")))??;

        Ok(Response::new(DeleteHistoryReply {
            deleted: deleted.cast(),
        }))
    }

    #[instrument(skip_all, level = Level::TRACE)]
    async fn rebuild_history(
        &self,
        _request: Request<RebuildHistoryRequest>,
    ) -> Result<Response<RebuildHistoryReply>, Status> {
        let search_settings = self.daemon_handle.settings().await.search.clone();
        let journal = self.journal.clone();
        // The rebuild runs in its own task. That way a client disconnect can't cancel it.
        tokio::spawn(
            async move { journal.rebuild(&search_settings).await }
                .instrument(tracing::Span::current()),
        )
        .await
        .map_err(|e| Status::internal(format!("rebuild did not complete: {e}")))??;

        Ok(Response::new(RebuildHistoryReply {}))
    }

    #[instrument(skip_all, level = Level::TRACE)]
    async fn compact_store(
        &self,
        _request: Request<CompactStoreRequest>,
    ) -> Result<Response<CompactStoreReply>, Status> {
        let rewritten = self
            .daemon_handle
            .store()
            .compact()
            .await
            .map_err(|e| Status::internal(format!("compact did not complete: {e}")))?;

        Ok(Response::new(CompactStoreReply { rewritten }))
    }

    #[instrument(skip_all, level = Level::TRACE)]
    async fn import_history(
        &self,
        request: Request<ImportHistoryRequest>,
    ) -> Result<Response<ImportHistoryReply>, Status> {
        let ImportHistoryRequest {
            shell,
            histfile,
            fish_history,
            wait,
        } = request.into_inner();
        let exe = std::env::current_exe().map_err(|e| Status::internal(e.to_string()))?;
        let service = self.clone();
        let (done, imported) = tokio::sync::oneshot::channel();
        // Spawned, so a client that stops waiting doesn't stop the import.
        tokio::spawn(async move {
            let result = service.import_shell_history(&exe, &shell, histfile, fish_history).await;
            if let Err(e) = &result {
                tracing::error!("{e}");
            }
            // Nobody is listening when the client didn't ask to wait, or stopped waiting.
            let _ = done.send(result);
        });
        if wait {
            imported
                .await
                .map_err(|_| Status::internal("the import stopped before it finished"))?
                .map_err(|e| Status::internal(e.to_string()))?;
        }
        Ok(Response::new(ImportHistoryReply {}))
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
            version: crate::VERSION.to_string(),
            pid: std::process::id(),
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

    /// Tells the daemon's parts that `atuin login` saved a session. Has nothing to do with
    /// history.
    #[instrument(skip_all, level = Level::TRACE)]
    async fn logged_in(
        &self,
        _request: Request<LoggedInRequest>,
    ) -> Result<Response<LoggedInReply>, Status> {
        self.daemon_handle.emit(DaemonEvent::LoggedIn);
        Ok(Response::new(LoggedInReply {}))
    }

    #[instrument(skip_all, level = Level::TRACE)]
    async fn register_command_output(
        &self,
        request: Request<RegisterCommandOutputRequest>,
    ) -> Result<Response<RegisterCommandOutputResponse>, Status> {
        let request = request.into_inner();
        let id = request.history_id()?;
        let capture = request.capture()?.into();
        let settings = self.daemon_handle.settings().await;
        let output = settings.output.clone();
        let files = settings.security.sensitive_files(&settings.key_path);
        drop(settings);
        let journal = self.journal.clone();
        // Spawned so a client disconnect cannot drop the call half-way.
        tokio::spawn(
            async move { journal.register_command_output(id, capture, &output, &files).await }
                .instrument(tracing::Span::current()),
        )
        .await
        .map_err(|e| Status::internal(format!("output registration did not complete: {e}")))??;
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

        Ok(Response::new(GetCommandOutputResponse::build(&capture.into(), request.output_ranges())))
    }
}
