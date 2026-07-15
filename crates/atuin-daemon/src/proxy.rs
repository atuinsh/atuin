//! `DaemonDatabase`: a client-side `atuin_client::database::Database`
//! implementation that forwards every call to the daemon over gRPC.
//!
//! This is the core of the "daemon owns all storage" architecture: the CLI
//! constructs a `DaemonDatabase` instead of a `Sqlite`, so it never opens the
//! local history database itself — the daemon (the sole owner) services the
//! request against its own `Sqlite`.

use atuin_client::{
    database::{Context, Database, OptFilters, Paged},
    history::{History, HistoryId, HistoryStats},
    settings::{FilterMode as DbFilterMode, SearchMode as DbSearchMode, Settings},
};
use sqlx::Result;
use time::OffsetDateTime;
use tonic::transport::{Channel, Endpoint, Uri};
use tower::service_fn;

use hyper_util::rt::TokioIo;

#[cfg(not(unix))]
use tokio::net::TcpStream;
#[cfg(unix)]
use tokio::net::UnixStream;

use crate::database::{
    BeforeRequest, DeleteRowsRequest, FilterMode, GetDupsRequest, HistoryCountRequest, ListRequest,
    LoadRequest, QueryHistoryRequest, RangeRequest, SaveBulkRequest, SaveRequest, SearchMode,
    SearchRequest, storage_database_client::StorageDatabaseClient,
};

/// Map any error coming off the wire into a `sqlx::Error`, which is the error
/// type the `Database` trait speaks. This keeps the proxy a drop-in for
/// `Sqlite` from the callers' point of view.
fn wire_err<E: std::error::Error + Send + Sync + 'static>(e: E) -> sqlx::Error {
    sqlx::Error::Configuration(Box::new(e))
}

fn ts_to_nanos(ts: OffsetDateTime) -> u64 {
    ts.unix_timestamp_nanos().max(0) as u64
}

/// A `Database` backed by a remote atuin daemon.
#[derive(Clone)]
pub struct DaemonDatabase {
    client: StorageDatabaseClient<Channel>,
}

impl DaemonDatabase {
    #[cfg(unix)]
    pub async fn from_settings(settings: &Settings) -> eyre::Result<Self> {
        Self::connect_unix(settings.daemon.socket_path.clone()).await
    }

    #[cfg(not(unix))]
    pub async fn from_settings(settings: &Settings) -> eyre::Result<Self> {
        Self::connect_tcp(settings.daemon.tcp_port).await
    }

    #[cfg(unix)]
    async fn connect_unix(path: String) -> eyre::Result<Self> {
        use eyre::WrapErr;

        let log_path = path.clone();
        let channel = Endpoint::try_from("http://atuin_local_daemon:0")?
            .connect_with_connector(service_fn(move |_: Uri| {
                let path = path.clone();
                async move {
                    Ok::<_, std::io::Error>(TokioIo::new(UnixStream::connect(path.clone()).await?))
                }
            }))
            .await
            .wrap_err_with(|| {
                format!("failed to connect to local atuin daemon at {log_path}. Is it running?")
            })?;

        Ok(Self {
            client: StorageDatabaseClient::new(channel),
        })
    }

    #[cfg(not(unix))]
    async fn connect_tcp(port: u64) -> eyre::Result<Self> {
        use eyre::WrapErr;

        let channel = Endpoint::try_from("http://atuin_local_daemon:0")?
            .connect_with_connector(service_fn(move |_: Uri| {
                let url = format!("127.0.0.1:{port}");
                async move {
                    Ok::<_, std::io::Error>(TokioIo::new(TcpStream::connect(url.clone()).await?))
                }
            }))
            .await
            .wrap_err_with(|| {
                format!(
                    "failed to connect to local atuin daemon at 127.0.0.1:{port}. Is it running?"
                )
            })?;

        Ok(Self {
            client: StorageDatabaseClient::new(channel),
        })
    }
}

#[tonic::async_trait]
impl Database for DaemonDatabase {
    async fn save(&self, h: &History) -> Result<()> {
        let mut client = self.client.clone();
        client
            .save(SaveRequest {
                record: Some(h.clone().into()),
            })
            .await
            .map_err(wire_err)?;
        Ok(())
    }

    async fn save_bulk(&self, h: &[History]) -> Result<()> {
        let mut client = self.client.clone();
        client
            .save_bulk(SaveBulkRequest {
                records: h.iter().cloned().map(Into::into).collect(),
            })
            .await
            .map_err(wire_err)?;
        Ok(())
    }

    async fn load(&self, id: &str) -> Result<Option<History>> {
        let mut client = self.client.clone();
        let reply = client
            .load(LoadRequest { id: id.to_string() })
            .await
            .map_err(wire_err)?
            .into_inner();
        Ok(reply.record.map(Into::into))
    }

    async fn list(
        &self,
        filters: &[DbFilterMode],
        context: &Context,
        max: Option<usize>,
        unique: bool,
        include_deleted: bool,
    ) -> Result<Vec<History>> {
        let mut client = self.client.clone();
        let reply = client
            .list(ListRequest {
                filters: filters.iter().map(|f| FilterMode::from(*f) as i32).collect(),
                context: Some(context.clone().into()),
                max: max.map(|m| m as u64),
                unique,
                include_deleted,
            })
            .await
            .map_err(wire_err)?
            .into_inner();
        Ok(reply.into_history())
    }

    async fn range(&self, from: OffsetDateTime, to: OffsetDateTime) -> Result<Vec<History>> {
        let mut client = self.client.clone();
        let reply = client
            .range(RangeRequest {
                from: ts_to_nanos(from),
                to: ts_to_nanos(to),
            })
            .await
            .map_err(wire_err)?
            .into_inner();
        Ok(reply.into_history())
    }

    async fn update(&self, h: &History) -> Result<()> {
        let mut client = self.client.clone();
        client
            .update(SaveRequest {
                record: Some(h.clone().into()),
            })
            .await
            .map_err(wire_err)?;
        Ok(())
    }

    async fn history_count(&self, include_deleted: bool) -> Result<i64> {
        let mut client = self.client.clone();
        let reply = client
            .history_count(HistoryCountRequest { include_deleted })
            .await
            .map_err(wire_err)?
            .into_inner();
        Ok(reply.count)
    }

    async fn last(&self) -> Result<Option<History>> {
        let mut client = self.client.clone();
        let reply = client
            .last(crate::database::Empty {})
            .await
            .map_err(wire_err)?
            .into_inner();
        Ok(reply.record.map(Into::into))
    }

    async fn before(&self, timestamp: OffsetDateTime, count: i64) -> Result<Vec<History>> {
        let mut client = self.client.clone();
        let reply = client
            .before(BeforeRequest {
                timestamp: ts_to_nanos(timestamp),
                count,
            })
            .await
            .map_err(wire_err)?
            .into_inner();
        Ok(reply.into_history())
    }

    async fn delete(&self, h: History) -> Result<()> {
        let mut client = self.client.clone();
        client
            .delete(SaveRequest {
                record: Some(h.into()),
            })
            .await
            .map_err(wire_err)?;
        Ok(())
    }

    async fn delete_rows(&self, ids: &[HistoryId]) -> Result<()> {
        let mut client = self.client.clone();
        client
            .delete_rows(DeleteRowsRequest {
                ids: ids.iter().map(|id| id.0.clone()).collect(),
            })
            .await
            .map_err(wire_err)?;
        Ok(())
    }

    async fn deleted(&self) -> Result<Vec<History>> {
        let mut client = self.client.clone();
        let reply = client
            .deleted(crate::database::Empty {})
            .await
            .map_err(wire_err)?
            .into_inner();
        Ok(reply.into_history())
    }

    async fn search(
        &self,
        search_mode: DbSearchMode,
        filter: DbFilterMode,
        context: &Context,
        query: &str,
        filter_options: OptFilters,
    ) -> Result<Vec<History>> {
        let mut client = self.client.clone();
        let reply = client
            .search(SearchRequest {
                search_mode: SearchMode::from(search_mode) as i32,
                filter: FilterMode::from(filter) as i32,
                context: Some(context.clone().into()),
                query: query.to_string(),
                filter_options: Some(filter_options.into()),
            })
            .await
            .map_err(wire_err)?
            .into_inner();
        Ok(reply.into_history())
    }

    async fn query_history(&self, query: &str) -> Result<Vec<History>> {
        let mut client = self.client.clone();
        let reply = client
            .query_history(QueryHistoryRequest {
                query: query.to_string(),
            })
            .await
            .map_err(wire_err)?
            .into_inner();
        Ok(reply.into_history())
    }

    async fn all_with_count(&self) -> Result<Vec<(History, i32)>> {
        let mut client = self.client.clone();
        let reply = client
            .all_with_count(crate::database::Empty {})
            .await
            .map_err(wire_err)?
            .into_inner();
        Ok(reply.into_history_with_count())
    }

    fn all_paged(&self, page_size: usize, include_deleted: bool, unique: bool) -> Paged {
        // Paged only needs a boxed Database; it drives itself via query_history,
        // which the proxy forwards. So this works transparently.
        Paged::new(self.clone_boxed(), page_size, include_deleted, unique)
    }

    async fn stats(&self, h: &History) -> Result<HistoryStats> {
        let mut client = self.client.clone();
        let reply = client
            .stats(SaveRequest {
                record: Some(h.clone().into()),
            })
            .await
            .map_err(wire_err)?
            .into_inner();
        Ok(reply.into())
    }

    async fn get_dups(&self, before: i64, dupkeep: u32) -> Result<Vec<History>> {
        let mut client = self.client.clone();
        let reply = client
            .get_dups(GetDupsRequest { before, dupkeep })
            .await
            .map_err(wire_err)?
            .into_inner();
        Ok(reply.into_history())
    }

    fn clone_boxed(&self) -> Box<dyn Database + 'static> {
        Box::new(self.clone())
    }
}
