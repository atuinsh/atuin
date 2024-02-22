use std::path::PathBuf;

use eyre::Result;
use tokio::net::UnixStream;
use tonic::transport::{Channel, Endpoint, Uri};
use tower::service_fn;

use atuin_client::history::History;

use crate::history::{
    history_client::HistoryClient as HistoryServiceClient, AddHistoryReply, AddHistoryRequest,
};

pub struct HistoryClient {
    client: HistoryServiceClient<Channel>,
}

// Wrap the grpc client
impl HistoryClient {
    pub async fn new(path: &'static str) -> Result<Self> {
        let channel = Endpoint::try_from("http://atuin_local_daemon:0")?
            .connect_with_connector(service_fn(|_: Uri| {
                let path = path.to_string();

                UnixStream::connect(path)
            }))
            .await?;

        let client = HistoryServiceClient::new(channel);

        Ok(HistoryClient { client })
    }

    pub async fn add_history(&mut self, h: History) -> Result<String> {
        let req = AddHistoryRequest { command: h.command };

        let resp = self.client.add_history(req).await?;

        Ok(resp.into_inner().id)
    }
}
