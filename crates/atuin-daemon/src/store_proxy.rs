//! `DaemonStore`: a client-side `atuin_client::record::store::Store` that
//! forwards every call to the daemon over gRPC. Records stay encrypted on the
//! wire; the daemon is a blob store and crypto stays client-side.

use atuin_client::record::store::Store;
use atuin_common::record::{EncryptedData, HostId, Record, RecordId, RecordStatus};
use atuin_client::settings::Settings;
use eyre::Result;
use tonic::transport::{Channel, Endpoint, Uri};
use tower::service_fn;

use hyper_util::rt::TokioIo;

#[cfg(not(unix))]
use tokio::net::TcpStream;
#[cfg(unix)]
use tokio::net::UnixStream;

use crate::store::{
    HostTagRequest, IdxRequest, KeyRequest, NextRequest, PushBatchRequest, ReEncryptRequest,
    RecordIdRequest, TagRequest, storage_store_client::StorageStoreClient,
};

fn hid(host: HostId) -> String {
    host.0.as_hyphenated().to_string()
}

/// A `Store` backed by a remote atuin daemon.
#[derive(Clone, Debug)]
pub struct DaemonStore {
    client: StorageStoreClient<Channel>,
}

impl DaemonStore {
    #[cfg(unix)]
    pub async fn from_settings(settings: &Settings) -> Result<Self> {
        Self::connect_unix(settings.daemon.socket_path.clone()).await
    }

    #[cfg(not(unix))]
    pub async fn from_settings(settings: &Settings) -> Result<Self> {
        Self::connect_tcp(settings.daemon.tcp_port).await
    }

    #[cfg(unix)]
    async fn connect_unix(path: String) -> Result<Self> {
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
            client: StorageStoreClient::new(channel),
        })
    }

    #[cfg(not(unix))]
    async fn connect_tcp(port: u64) -> Result<Self> {
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
                format!("failed to connect to local atuin daemon at 127.0.0.1:{port}. Is it running?")
            })?;
        Ok(Self {
            client: StorageStoreClient::new(channel),
        })
    }
}

#[tonic::async_trait]
impl Store for DaemonStore {
    async fn push_batch(
        &self,
        records: &mut (dyn Iterator<Item = &Record<EncryptedData>> + Send),
    ) -> Result<()> {
        let records = records.map(|r| r.clone().into()).collect();
        let mut client = self.client.clone();
        client.push_batch(PushBatchRequest { records }).await?;
        Ok(())
    }

    async fn get(&self, id: RecordId) -> Result<Record<EncryptedData>> {
        let mut client = self.client.clone();
        let reply = client
            .get(RecordIdRequest {
                id: id.0.as_hyphenated().to_string(),
            })
            .await?
            .into_inner();
        Ok(reply.into())
    }

    async fn delete(&self, id: RecordId) -> Result<()> {
        let mut client = self.client.clone();
        client
            .delete(RecordIdRequest {
                id: id.0.as_hyphenated().to_string(),
            })
            .await?;
        Ok(())
    }

    async fn delete_all(&self) -> Result<()> {
        let mut client = self.client.clone();
        client.delete_all(crate::store::Empty {}).await?;
        Ok(())
    }

    async fn len_all(&self) -> Result<u64> {
        let mut client = self.client.clone();
        Ok(client.len_all(crate::store::Empty {}).await?.into_inner().count)
    }

    async fn len(&self, host: HostId, tag: &str) -> Result<u64> {
        let mut client = self.client.clone();
        Ok(client
            .len(HostTagRequest {
                host: hid(host),
                tag: tag.to_string(),
            })
            .await?
            .into_inner()
            .count)
    }

    async fn len_tag(&self, tag: &str) -> Result<u64> {
        let mut client = self.client.clone();
        Ok(client
            .len_tag(TagRequest {
                tag: tag.to_string(),
            })
            .await?
            .into_inner()
            .count)
    }

    async fn last(&self, host: HostId, tag: &str) -> Result<Option<Record<EncryptedData>>> {
        let mut client = self.client.clone();
        let reply = client
            .last(HostTagRequest {
                host: hid(host),
                tag: tag.to_string(),
            })
            .await?
            .into_inner();
        Ok(reply.record.map(Into::into))
    }

    async fn first(&self, host: HostId, tag: &str) -> Result<Option<Record<EncryptedData>>> {
        let mut client = self.client.clone();
        let reply = client
            .first(HostTagRequest {
                host: hid(host),
                tag: tag.to_string(),
            })
            .await?
            .into_inner();
        Ok(reply.record.map(Into::into))
    }

    async fn re_encrypt(&self, old_key: &[u8; 32], new_key: &[u8; 32]) -> Result<()> {
        let mut client = self.client.clone();
        client
            .re_encrypt(ReEncryptRequest {
                old_key: old_key.to_vec(),
                new_key: new_key.to_vec(),
            })
            .await?;
        Ok(())
    }

    async fn verify(&self, key: &[u8; 32]) -> Result<()> {
        let mut client = self.client.clone();
        client
            .verify(KeyRequest {
                key: key.to_vec(),
            })
            .await?;
        Ok(())
    }

    async fn purge(&self, key: &[u8; 32]) -> Result<()> {
        let mut client = self.client.clone();
        client
            .purge(KeyRequest {
                key: key.to_vec(),
            })
            .await?;
        Ok(())
    }

    async fn next(
        &self,
        host: HostId,
        tag: &str,
        idx: atuin_common::record::RecordIdx,
        limit: u64,
    ) -> Result<Vec<Record<EncryptedData>>> {
        let mut client = self.client.clone();
        let reply = client
            .next(NextRequest {
                host: hid(host),
                tag: tag.to_string(),
                idx,
                limit,
            })
            .await?
            .into_inner();
        Ok(reply.into_records())
    }

    async fn idx(
        &self,
        host: HostId,
        tag: &str,
        idx: atuin_common::record::RecordIdx,
    ) -> Result<Option<Record<EncryptedData>>> {
        let mut client = self.client.clone();
        let reply = client
            .idx(IdxRequest {
                host: hid(host),
                tag: tag.to_string(),
                idx,
            })
            .await?
            .into_inner();
        Ok(reply.record.map(Into::into))
    }

    async fn status(&self) -> Result<RecordStatus> {
        let mut client = self.client.clone();
        let reply = client.status(crate::store::Empty {}).await?.into_inner();
        Ok(reply.into())
    }

    async fn all_tagged(&self, tag: &str) -> Result<Vec<Record<EncryptedData>>> {
        let mut client = self.client.clone();
        let reply = client
            .all_tagged(TagRequest {
                tag: tag.to_string(),
            })
            .await?
            .into_inner();
        Ok(reply.into_records())
    }
}
