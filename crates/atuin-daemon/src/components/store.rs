//! StorageStore gRPC service.
//!
//! A thin adapter exposing the daemon's owned record `SqliteStore` over gRPC by
//! delegating to the `atuin_client::record::store::Store` trait. Records stay
//! encrypted; the daemon is a blob store and crypto stays client-side.

use atuin_client::record::{sqlite_store::SqliteStore, store::Store};
use atuin_common::record::{HostId, RecordId};
use tonic::{Request, Response, Status};
use uuid::Uuid;

use crate::store::{
    Count, Empty, HostTagRequest, IdxRequest, KeyRequest, NextRequest, OptionalRecord,
    PushBatchRequest, ReEncryptRequest, RecordIdRequest, Records, RecordStatusReply, TagRequest,
    storage_store_server::{StorageStore, StorageStoreServer},
};

pub struct StorageStoreService {
    store: SqliteStore,
}

impl StorageStoreService {
    pub fn new(store: SqliteStore) -> Self {
        Self { store }
    }

    pub fn into_server(self) -> StorageStoreServer<Self> {
        StorageStoreServer::new(self)
    }
}

fn internal<E: std::fmt::Display>(e: E) -> Status {
    Status::internal(e.to_string())
}

fn host_id(s: &str) -> HostId {
    HostId(Uuid::parse_str(s).unwrap_or(Uuid::nil()))
}

fn record_id(s: &str) -> RecordId {
    RecordId(Uuid::parse_str(s).unwrap_or(Uuid::nil()))
}

fn key32(bytes: &[u8]) -> Result<[u8; 32], Status> {
    bytes
        .try_into()
        .map_err(|_| Status::invalid_argument("key must be 32 bytes"))
}

#[tonic::async_trait]
impl StorageStore for StorageStoreService {
    async fn push_batch(
        &self,
        request: Request<PushBatchRequest>,
    ) -> Result<Response<Empty>, Status> {
        let records: Vec<_> = request
            .into_inner()
            .records
            .into_iter()
            .map(Into::into)
            .collect();
        self.store
            .push_batch(&mut records.iter())
            .await
            .map_err(internal)?;
        Ok(Response::new(Empty {}))
    }

    async fn get(
        &self,
        request: Request<RecordIdRequest>,
    ) -> Result<Response<crate::store::EncryptedRecord>, Status> {
        let record = self
            .store
            .get(record_id(&request.into_inner().id))
            .await
            .map_err(internal)?;
        Ok(Response::new(record.into()))
    }

    async fn delete(&self, request: Request<RecordIdRequest>) -> Result<Response<Empty>, Status> {
        self.store
            .delete(record_id(&request.into_inner().id))
            .await
            .map_err(internal)?;
        Ok(Response::new(Empty {}))
    }

    async fn delete_all(&self, _request: Request<Empty>) -> Result<Response<Empty>, Status> {
        self.store.delete_all().await.map_err(internal)?;
        Ok(Response::new(Empty {}))
    }

    async fn len_all(&self, _request: Request<Empty>) -> Result<Response<Count>, Status> {
        let count = self.store.len_all().await.map_err(internal)?;
        Ok(Response::new(Count { count }))
    }

    async fn len(&self, request: Request<HostTagRequest>) -> Result<Response<Count>, Status> {
        let req = request.into_inner();
        let count = self
            .store
            .len(host_id(&req.host), &req.tag)
            .await
            .map_err(internal)?;
        Ok(Response::new(Count { count }))
    }

    async fn len_tag(&self, request: Request<TagRequest>) -> Result<Response<Count>, Status> {
        let count = self
            .store
            .len_tag(&request.into_inner().tag)
            .await
            .map_err(internal)?;
        Ok(Response::new(Count { count }))
    }

    async fn last(
        &self,
        request: Request<HostTagRequest>,
    ) -> Result<Response<OptionalRecord>, Status> {
        let req = request.into_inner();
        let record = self
            .store
            .last(host_id(&req.host), &req.tag)
            .await
            .map_err(internal)?;
        Ok(Response::new(OptionalRecord {
            record: record.map(Into::into),
        }))
    }

    async fn first(
        &self,
        request: Request<HostTagRequest>,
    ) -> Result<Response<OptionalRecord>, Status> {
        let req = request.into_inner();
        let record = self
            .store
            .first(host_id(&req.host), &req.tag)
            .await
            .map_err(internal)?;
        Ok(Response::new(OptionalRecord {
            record: record.map(Into::into),
        }))
    }

    async fn re_encrypt(
        &self,
        request: Request<ReEncryptRequest>,
    ) -> Result<Response<Empty>, Status> {
        let req = request.into_inner();
        self.store
            .re_encrypt(&key32(&req.old_key)?, &key32(&req.new_key)?)
            .await
            .map_err(internal)?;
        Ok(Response::new(Empty {}))
    }

    async fn verify(&self, request: Request<KeyRequest>) -> Result<Response<Empty>, Status> {
        self.store
            .verify(&key32(&request.into_inner().key)?)
            .await
            .map_err(internal)?;
        Ok(Response::new(Empty {}))
    }

    async fn purge(&self, request: Request<KeyRequest>) -> Result<Response<Empty>, Status> {
        self.store
            .purge(&key32(&request.into_inner().key)?)
            .await
            .map_err(internal)?;
        Ok(Response::new(Empty {}))
    }

    async fn next(&self, request: Request<NextRequest>) -> Result<Response<Records>, Status> {
        let req = request.into_inner();
        let records = self
            .store
            .next(host_id(&req.host), &req.tag, req.idx, req.limit)
            .await
            .map_err(internal)?;
        Ok(Response::new(Records::from_records(records)))
    }

    async fn idx(&self, request: Request<IdxRequest>) -> Result<Response<OptionalRecord>, Status> {
        let req = request.into_inner();
        let record = self
            .store
            .idx(host_id(&req.host), &req.tag, req.idx)
            .await
            .map_err(internal)?;
        Ok(Response::new(OptionalRecord {
            record: record.map(Into::into),
        }))
    }

    async fn status(
        &self,
        _request: Request<Empty>,
    ) -> Result<Response<RecordStatusReply>, Status> {
        let status = self.store.status().await.map_err(internal)?;
        Ok(Response::new(status.into()))
    }

    async fn all_tagged(&self, request: Request<TagRequest>) -> Result<Response<Records>, Status> {
        let records = self
            .store
            .all_tagged(&request.into_inner().tag)
            .await
            .map_err(internal)?;
        Ok(Response::new(Records::from_records(records)))
    }
}
