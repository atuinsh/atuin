//! Utility library for interacting with the `pty-proxy` over its native IPC channel.
use std::path::PathBuf;
use std::time::Duration;

use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::time::timeout;

use crate::ipc::domain::{
    DumpScreenReq, GoodbyeReq, HelloReq, IsRequest, PROTOCOL_VERSION, Rep, Req,
};
use crate::ipc::wire::{self, EncodeError, Header, HeaderParseError};
use crate::screen::ScreenSnapshot;

#[derive(Debug, Error)]
pub enum IpcError {
    #[error("io error talking to pty-proxy: {0}")]
    Io(std::io::Error),

    #[error("timed out talking to pty-proxy")]
    Timeout,

    #[error("failed to encode message: {0}")]
    Encode(EncodeError),

    #[error("failed to parse header: {0}")]
    Header(HeaderParseError),

    #[error("failed to decode message: {0}")]
    Decode(postcard::Error),

    #[error("server sent an unexpected reply")]
    UnexpectedReply,
}

#[derive(Debug, Error)]
pub enum IpcConnectError {
    #[error("failed to connect to pty-proxy at {path}: {source}")]
    Connect {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error("timed out connecting to pty-proxy")]
    Timeout,

    #[error("protocol version mismatch: ours={ours}, theirs={theirs}")]
    ProtocolMismatch {
        ours: u32,
        theirs: u32,
    },

    #[error("handshake with pty-proxy failed: {0}")]
    Handshake(#[from] IpcError),
}

#[derive(Debug, Clone)]
pub struct IpcClient {
    sock_path: PathBuf,
    connect_timeout: Duration,
    request_timeout: Duration,
}

impl IpcClient {
    const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_millis(100);
    const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(2);

    pub fn new(sock_path: impl Into<PathBuf>) -> Self {
        Self {
            sock_path: sock_path.into(),
            connect_timeout: Self::DEFAULT_CONNECT_TIMEOUT,
            request_timeout: Self::DEFAULT_REQUEST_TIMEOUT,
        }
    }

    #[must_use]
    pub fn with_connect_timeout(mut self, timeout: Duration) -> Self {
        self.connect_timeout = timeout;
        self
    }

    #[must_use]
    pub fn with_request_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = timeout;
        self
    }

    pub async fn connect(self) -> Result<IpcConnection, IpcConnectError> {
        let stream = timeout(self.connect_timeout, UnixStream::connect(&self.sock_path))
            .await
            .map_err(|_| IpcConnectError::Timeout)?
            .map_err(|source| IpcConnectError::Connect {
                path: self.sock_path.clone(),
                source,
            })?;

        let mut conn = IpcConnection {
            stream,
            request_timeout: self.request_timeout,
            scratch: Vec::new(),
        };

        let rep = conn
            .call(HelloReq {
                version: PROTOCOL_VERSION,
            })
            .await?;
        if rep.version != PROTOCOL_VERSION {
            return Err(IpcConnectError::ProtocolMismatch {
                ours: PROTOCOL_VERSION,
                theirs: rep.version,
            });
        }

        Ok(conn)
    }
}

#[derive(Debug)]
pub struct IpcConnection {
    stream: UnixStream,
    request_timeout: Duration,
    scratch: Vec<u8>,
}

impl IpcConnection {
    pub async fn dump_screen(&mut self) -> Result<ScreenSnapshot, IpcError> {
        Ok(self.call(DumpScreenReq).await?.screen)
    }

    pub async fn close(mut self) -> Result<(), IpcError> {
        self.call(GoodbyeReq).await?;
        Ok(())
    }

    async fn call<R: IsRequest>(&mut self, op: R) -> Result<R::Rep, IpcError> {
        let req: Req = op.into();
        let framed = wire::try_encode(&req).map_err(IpcError::Encode)?;
        let request_timeout = self.request_timeout;

        let rep = timeout(request_timeout, exchange(&mut self.stream, &mut self.scratch, &framed))
            .await
            .map_err(|_| IpcError::Timeout)??;

        R::Rep::try_from(rep).map_err(|_| IpcError::UnexpectedReply)
    }
}

async fn exchange(
    stream: &mut UnixStream,
    scratch: &mut Vec<u8>,
    framed: &[u8],
) -> Result<Rep, IpcError> {
    stream.write_all(framed).await.map_err(IpcError::Io)?;
    stream.flush().await.map_err(IpcError::Io)?;

    let mut header_bytes = [0u8; Header::SERIALIZED_LEN];
    stream.read_exact(&mut header_bytes).await.map_err(IpcError::Io)?;

    let header = Header::parse(header_bytes).map_err(IpcError::Header)?;
    let body_len = (header.message_width as usize).saturating_sub(Header::SERIALIZED_LEN);
    if scratch.len() < body_len {
        scratch.resize(body_len, 0);
    }
    let buf = &mut scratch[..body_len];
    stream.read_exact(buf).await.map_err(IpcError::Io)?;

    wire::decode_body::<Rep>(buf).map_err(IpcError::Decode)
}
