use std::error::Error;
use std::path::PathBuf;

use eyre::Result;
use tokio::net::UnixListener;
use tokio_stream::wrappers::UnixListenerStream;
use tonic::{transport::Server, Request, Response, Status};

use crate::history::history_server::{History, HistoryServer};

use crate::history::{AddHistoryReply, AddHistoryRequest};

#[derive(Debug, Default)]
pub struct HistoryService {}

#[tonic::async_trait()]
impl History for HistoryService {
    async fn add_history(
        &self,
        request: Request<AddHistoryRequest>,
    ) -> Result<Response<AddHistoryReply>, Status> {
        println!("request to add {}", request.into_inner().command);

        let reply = AddHistoryReply {
            id: "foo".to_string(),
        };

        Ok(Response::new(reply))
    }
}

// break the above down when we end up with multiple services

/// Listen on a unix socket
/// Pass the path to the socket
pub async fn listen(path: PathBuf) -> Result<()> {
    let history = HistoryService::default();

    let uds = UnixListener::bind(path)?;
    let uds_stream = UnixListenerStream::new(uds);

    Server::builder()
        .add_service(HistoryServer::new(history))
        .serve_with_incoming(uds_stream)
        .await?;

    Ok(())
}
