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

    #[error("pty-proxy sent malformed screen data")]
    MalformedScreen,
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
    protocol: Option<u32>,
}

impl IpcClient {
    const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_millis(100);
    const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(2);

    pub fn new(sock_path: impl Into<PathBuf>) -> Self {
        Self {
            sock_path: sock_path.into(),
            connect_timeout: Self::DEFAULT_CONNECT_TIMEOUT,
            request_timeout: Self::DEFAULT_REQUEST_TIMEOUT,
            protocol: None,
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

    /// The IPC protocol version the proxy advertised via the
    /// `ATUIN_PTY_PROXY_PROTOCOL` environment variable. `None` means the
    /// variable was absent: a legacy proxy that only speaks the V0 push
    /// protocol.
    #[must_use]
    pub fn with_protocol(mut self, protocol: Option<u32>) -> Self {
        self.protocol = protocol;
        self
    }

    pub async fn connect(self) -> Result<IpcConnection, IpcConnectError> {
        match self.protocol {
            // The proxy advertised the framed protocol via `ATUIN_PTY_PROXY_PROTOCOL`.          Some(_) => self.connect_v1().await.map(IpcConnection::V1),
            // No advertisement: a legacy proxy that pushes a raw screen dump on
            // connect. Nothing to negotiate; connect lazily on each dump.
            #[allow(deprecated, reason = "legacy pty-proxy ipc protocol")]
            None => Ok(IpcConnection::V0(V0Connection {
                sock_path: self.sock_path,
                connect_timeout: self.connect_timeout,
                request_timeout: self.request_timeout,
            })),
        }
    }

    async fn connect_v1(&self) -> Result<V1Connection, IpcConnectError> {
        let stream = timeout(self.connect_timeout, UnixStream::connect(&self.sock_path))
            .await
            .map_err(|_| IpcConnectError::Timeout)?
            .map_err(|source| IpcConnectError::Connect {
                path: self.sock_path.clone(),
                source,
            })?;

        let mut conn = V1Connection {
            stream,
            request_timeout: self.request_timeout,
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

/// A live connection to a pty-proxy, dispatching to the protocol version the
/// proxy speaks.
#[derive(Debug)]
pub enum IpcConnection {
    /// Legacy proxy (v18.20.1 and earlier): pushes a raw screen dump on connect.
    #[allow(deprecated, reason = "legacy pty-proxy ipc protocol")]
    V0(V0Connection),
    /// Framed request/response protocol.
    V1(V1Connection),
}

impl IpcConnection {
    pub async fn dump_screen(&mut self) -> Result<ScreenSnapshot, IpcError> {
        match self {
            #[allow(deprecated, reason = "legacy pty-proxy ipc protocol")]
            Self::V0(conn) => conn.dump_screen().await,
            Self::V1(conn) => conn.dump_screen().await,
        }
    }

    pub async fn close(self) -> Result<(), IpcError> {
        match self {
            #[allow(deprecated, reason = "legacy pty-proxy ipc protocol")]
            Self::V0(_) => Ok(()),
            Self::V1(conn) => conn.close().await,
        }
    }
}

/// Legacy (V0) connection. The old proxy has no request/response protocol: on
/// each connect it writes a raw screen dump and closes, so every `dump_screen`
/// opens a fresh connection and reads to EOF.
#[deprecated]
#[derive(Debug)]
pub struct V0Connection {
    sock_path: PathBuf,
    connect_timeout: Duration,
    request_timeout: Duration,
}

#[allow(deprecated, reason = "legacy pty-proxy ipc protocol")]
impl V0Connection {
    async fn dump_screen(&self) -> Result<ScreenSnapshot, IpcError> {
        let mut stream = timeout(self.connect_timeout, UnixStream::connect(&self.sock_path))
            .await
            .map_err(|_| IpcError::Timeout)?
            .map_err(IpcError::Io)?;

        let mut data = Vec::new();
        timeout(self.request_timeout, stream.read_to_end(&mut data))
            .await
            .map_err(|_| IpcError::Timeout)?
            .map_err(IpcError::Io)?;

        #[allow(deprecated, reason = "legacy pty-proxy ipc protocol")]
        parse_v0_screen(&data)
    }
}

/// Parse the legacy screen-dump wire format: a `[rows][cols][cursor_row]
/// [cursor_col]` big-endian `u16` head, then each row as a big-endian `u32`
/// length followed by that many UTF-8 bytes.
#[deprecated]
fn parse_v0_screen(data: &[u8]) -> Result<ScreenSnapshot, IpcError> {
    let head: [u8; 8] =
        data.get(..8).and_then(|h| h.try_into().ok()).ok_or(IpcError::MalformedScreen)?;
    let screen_dims =
        (u16::from_be_bytes([head[0], head[1]]), u16::from_be_bytes([head[2], head[3]]));
    let cursor_pos =
        (u16::from_be_bytes([head[4], head[5]]), u16::from_be_bytes([head[6], head[7]]));

    let mut rest = &data[8..];
    let mut rows = Vec::new();
    while !rest.is_empty() {
        let (len_bytes, tail) = rest.split_first_chunk::<4>().ok_or(IpcError::MalformedScreen)?;
        let len = u32::from_be_bytes(*len_bytes) as usize;
        if tail.len() < len {
            return Err(IpcError::MalformedScreen);
        }
        let (row_bytes, tail) = tail.split_at(len);
        rows.push(String::from_utf8(row_bytes.to_vec()).map_err(|_| IpcError::MalformedScreen)?);
        rest = tail;
    }

    Ok(ScreenSnapshot::from_parts(screen_dims, cursor_pos, rows))
}

/// Framed request/response connection (V1). Persistent: one stream, many calls.
#[derive(Debug)]
pub struct V1Connection {
    stream: UnixStream,
    request_timeout: Duration,
}

impl V1Connection {
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

        let rep = timeout(request_timeout, self.exchange(&framed))
            .await
            .map_err(|_| IpcError::Timeout)??;

        R::Rep::try_from(rep).map_err(|_| IpcError::UnexpectedReply)
    }

    async fn exchange(&mut self, framed: &[u8]) -> Result<Rep, IpcError> {
        self.stream.write_all(framed).await.map_err(IpcError::Io)?;
        self.stream.flush().await.map_err(IpcError::Io)?;

        let mut header_bytes = [0u8; Header::SERIALIZED_LEN];
        self.stream.read_exact(&mut header_bytes).await.map_err(IpcError::Io)?;

        let header = Header::parse(header_bytes).map_err(IpcError::Header)?;
        let body_len = (header.message_width as usize).saturating_sub(Header::SERIALIZED_LEN);
        let mut buf = vec![0u8; body_len];
        self.stream.read_exact(&mut buf).await.map_err(IpcError::Io)?;

        wire::decode_body::<Rep>(&buf).map_err(IpcError::Decode)
    }
}
