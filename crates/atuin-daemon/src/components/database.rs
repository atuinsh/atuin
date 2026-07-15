//! StorageDatabase gRPC service.
//!
//! A thin adapter that exposes the daemon's owned history `Sqlite` database
//! over gRPC by delegating each call to the `atuin_client::database::Database`
//! trait. This lets the CLI act as a pure client and never open the local
//! SQLite database itself.

use atuin_client::{
    database::{Database, OptFilters},
    history::HistoryId,
    settings::{FilterMode as DbFilterMode, SearchMode as DbSearchMode},
};
use time::OffsetDateTime;
use tonic::{Request, Response, Status};

use crate::database::{
    Context, Empty, FilterMode, HistoryCountReply, HistoryCountRequest, HistoryRecord, ListRequest,
    LoadRequest, OptionalRecord, QueryHistoryRequest, RangeRequest, Records, SaveBulkRequest,
    SaveRequest, SearchMode, SearchRequest,
    storage_database_server::{StorageDatabase, StorageDatabaseServer},
    BeforeRequest, DeleteRowsRequest, GetDupsRequest,
};

use atuin_client::database::Sqlite as HistoryDatabase;

/// The StorageDatabase gRPC service, backed by the daemon's `Sqlite`.
pub struct StorageDatabaseService {
    db: HistoryDatabase,
}

impl StorageDatabaseService {
    pub fn new(db: HistoryDatabase) -> Self {
        Self { db }
    }

    pub fn into_server(self) -> StorageDatabaseServer<Self> {
        StorageDatabaseServer::new(self)
    }
}

fn internal<E: std::fmt::Display>(e: E) -> Status {
    Status::internal(e.to_string())
}

fn nanos_to_ts(nanos: u64) -> Result<OffsetDateTime, Status> {
    OffsetDateTime::from_unix_timestamp_nanos(nanos as i128)
        .map_err(|_| Status::invalid_argument("invalid timestamp"))
}

fn require_record(record: Option<HistoryRecord>) -> Result<HistoryRecord, Status> {
    record.ok_or_else(|| Status::invalid_argument("missing history record"))
}

fn ctx(context: Option<Context>) -> atuin_client::database::Context {
    context.map(Into::into).unwrap_or_else(|| atuin_client::database::Context {
        session: String::new(),
        cwd: String::new(),
        hostname: String::new(),
        host_id: String::new(),
        git_root: None,
    })
}

#[tonic::async_trait]
impl StorageDatabase for StorageDatabaseService {
    async fn save(&self, request: Request<SaveRequest>) -> Result<Response<Empty>, Status> {
        let record = require_record(request.into_inner().record)?;
        self.db.save(&record.into()).await.map_err(internal)?;
        Ok(Response::new(Empty {}))
    }

    async fn save_bulk(
        &self,
        request: Request<SaveBulkRequest>,
    ) -> Result<Response<Empty>, Status> {
        let history: Vec<_> = request
            .into_inner()
            .records
            .into_iter()
            .map(Into::into)
            .collect();
        self.db.save_bulk(&history).await.map_err(internal)?;
        Ok(Response::new(Empty {}))
    }

    async fn load(&self, request: Request<LoadRequest>) -> Result<Response<OptionalRecord>, Status> {
        let id = request.into_inner().id;
        let record = self.db.load(&id).await.map_err(internal)?;
        Ok(Response::new(OptionalRecord {
            record: record.map(Into::into),
        }))
    }

    async fn list(&self, request: Request<ListRequest>) -> Result<Response<Records>, Status> {
        let req = request.into_inner();
        let filters: Vec<DbFilterMode> = req
            .filters
            .into_iter()
            .filter_map(|f| FilterMode::try_from(f).ok())
            .map(DbFilterMode::from)
            .collect();
        let context = ctx(req.context);
        let history = self
            .db
            .list(
                &filters,
                &context,
                req.max.map(|m| m as usize),
                req.unique,
                req.include_deleted,
            )
            .await
            .map_err(internal)?;
        Ok(Response::new(Records::from_history(history)))
    }

    async fn range(&self, request: Request<RangeRequest>) -> Result<Response<Records>, Status> {
        let req = request.into_inner();
        let history = self
            .db
            .range(nanos_to_ts(req.from)?, nanos_to_ts(req.to)?)
            .await
            .map_err(internal)?;
        Ok(Response::new(Records::from_history(history)))
    }

    async fn update(&self, request: Request<SaveRequest>) -> Result<Response<Empty>, Status> {
        let record = require_record(request.into_inner().record)?;
        self.db.update(&record.into()).await.map_err(internal)?;
        Ok(Response::new(Empty {}))
    }

    async fn history_count(
        &self,
        request: Request<HistoryCountRequest>,
    ) -> Result<Response<HistoryCountReply>, Status> {
        let count = self
            .db
            .history_count(request.into_inner().include_deleted)
            .await
            .map_err(internal)?;
        Ok(Response::new(HistoryCountReply { count }))
    }

    async fn last(&self, _request: Request<Empty>) -> Result<Response<OptionalRecord>, Status> {
        let record = self.db.last().await.map_err(internal)?;
        Ok(Response::new(OptionalRecord {
            record: record.map(Into::into),
        }))
    }

    async fn before(&self, request: Request<BeforeRequest>) -> Result<Response<Records>, Status> {
        let req = request.into_inner();
        let history = self
            .db
            .before(nanos_to_ts(req.timestamp)?, req.count)
            .await
            .map_err(internal)?;
        Ok(Response::new(Records::from_history(history)))
    }

    async fn delete(&self, request: Request<SaveRequest>) -> Result<Response<Empty>, Status> {
        let record = require_record(request.into_inner().record)?;
        self.db.delete(record.into()).await.map_err(internal)?;
        Ok(Response::new(Empty {}))
    }

    async fn delete_rows(
        &self,
        request: Request<DeleteRowsRequest>,
    ) -> Result<Response<Empty>, Status> {
        let ids: Vec<HistoryId> = request
            .into_inner()
            .ids
            .into_iter()
            .map(HistoryId)
            .collect();
        self.db.delete_rows(&ids).await.map_err(internal)?;
        Ok(Response::new(Empty {}))
    }

    async fn deleted(&self, _request: Request<Empty>) -> Result<Response<Records>, Status> {
        let history = self.db.deleted().await.map_err(internal)?;
        Ok(Response::new(Records::from_history(history)))
    }

    async fn search(&self, request: Request<SearchRequest>) -> Result<Response<Records>, Status> {
        let req = request.into_inner();
        let search_mode: DbSearchMode = SearchMode::try_from(req.search_mode)
            .map(DbSearchMode::from)
            .unwrap_or(DbSearchMode::Fuzzy);
        let filter: DbFilterMode = FilterMode::try_from(req.filter)
            .map(DbFilterMode::from)
            .unwrap_or(DbFilterMode::Global);
        let context = ctx(req.context);
        let filter_options: OptFilters = req.filter_options.map(Into::into).unwrap_or_default();

        let history = self
            .db
            .search(search_mode, filter, &context, &req.query, filter_options)
            .await
            .map_err(internal)?;
        Ok(Response::new(Records::from_history(history)))
    }

    async fn query_history(
        &self,
        request: Request<QueryHistoryRequest>,
    ) -> Result<Response<Records>, Status> {
        let history = self
            .db
            .query_history(&request.into_inner().query)
            .await
            .map_err(internal)?;
        Ok(Response::new(Records::from_history(history)))
    }

    async fn get_dups(&self, request: Request<GetDupsRequest>) -> Result<Response<Records>, Status> {
        let req = request.into_inner();
        let history = self
            .db
            .get_dups(req.before, req.dupkeep)
            .await
            .map_err(internal)?;
        Ok(Response::new(Records::from_history(history)))
    }
}
