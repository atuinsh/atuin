use std::path::PathBuf;
use std::time::Duration;

use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::time::timeout;

use crate::ipc::domain::{
    DumpScreenRep, DumpScreenReq, GoodbyeRep, GoodbyeReq, HelloRep, HelloReq, PROTOCOL_VERSION,
    Rep, Req,
};
use crate::ipc::wire::{self, FrameError};
use crate::screen::ScreenSnapshot;

#[derive(Debug, Error)]
pub enum IpcError {
    #[error("failed to connect to pty-proxy at {path}: {source}")]
    Connect {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error("io error talking to pty-proxy: {0}")]
    Io(std::io::Error),

    #[error("timed out talking to pty-proxy")]
    Timeout,

    #[error("failed to frame message: {0}")]
    Frame(FrameError),

    #[error("protocol version mismatch: ours={ours}, theirs={theirs}")]
    ProtocolMismatch {
        ours: u32,
        theirs: u32,
    },

    #[error("server sent an unexpected reply")]
    UnexpectedReply,
}

#[derive(Debug, Clone)]
pub struct IpcClient {
    sock_path: PathBuf,
    connect_timeout: Duration,
    request_timeout: Duration,
}

impl IpcClient {
    const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_millis(500);
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

    pub async fn connect(self) -> Result<IpcConnection, IpcError> {
        let stream = timeout(self.connect_timeout, UnixStream::connect(&self.sock_path))
            .await
            .map_err(|_| IpcError::Timeout)?
            .map_err(|source| IpcError::Connect {
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
            return Err(IpcError::ProtocolMismatch {
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

    async fn call<O: Op>(&mut self, op: O) -> Result<O::Rep, IpcError> {
        let framed = wire::encode_frame(&op.into_req()).map_err(IpcError::Frame)?;
        let request_timeout = self.request_timeout;

        let rep = timeout(request_timeout, exchange(&mut self.stream, &mut self.scratch, &framed))
            .await
            .map_err(|_| IpcError::Timeout)??;

        O::from_rep(rep).ok_or(IpcError::UnexpectedReply)
    }
}

async fn exchange(
    stream: &mut UnixStream,
    scratch: &mut Vec<u8>,
    framed: &[u8],
) -> Result<Rep, IpcError> {
    stream.write_all(framed).await.map_err(IpcError::Io)?;
    stream.flush().await.map_err(IpcError::Io)?;

    let mut len_bytes = [0u8; wire::LEN_PREFIX_BYTES];
    stream.read_exact(&mut len_bytes).await.map_err(IpcError::Io)?;

    let len = wire::parse_len(len_bytes).map_err(IpcError::Frame)?;
    if scratch.len() < len {
        scratch.resize(len, 0);
    }
    let buf = &mut scratch[..len];
    stream.read_exact(buf).await.map_err(IpcError::Io)?;

    wire::decode_body::<Rep>(buf).map_err(IpcError::Frame)
}

trait Op {
    type Rep;
    fn into_req(self) -> Req;
    fn from_rep(rep: Rep) -> Option<Self::Rep>;
}

impl Op for HelloReq {
    type Rep = HelloRep;
    fn into_req(self) -> Req {
        Req::Hello(self)
    }
    fn from_rep(rep: Rep) -> Option<HelloRep> {
        match rep {
            Rep::Hello(rep) => Some(rep),
            _ => None,
        }
    }
}

impl Op for DumpScreenReq {
    type Rep = DumpScreenRep;
    fn into_req(self) -> Req {
        Req::DumpScreen(self)
    }
    fn from_rep(rep: Rep) -> Option<DumpScreenRep> {
        match rep {
            Rep::DumpScreenRep(rep) => Some(rep),
            _ => None,
        }
    }
}

impl Op for GoodbyeReq {
    type Rep = GoodbyeRep;
    fn into_req(self) -> Req {
        Req::Goodbye(self)
    }
    fn from_rep(rep: Rep) -> Option<GoodbyeRep> {
        match rep {
            Rep::Goodbye(rep) => Some(rep),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read as _, Write as _};
    use std::os::unix::net::UnixListener as StdUnixListener;
    use std::path::Path;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::mpsc::sync_channel;

    use super::*;
    use crate::ipc::controller::IpcController;
    use crate::ipc::server::IpcServer;
    use crate::screen::{self, Msg};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    fn temp_sock() -> PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("atuin-ipc-test-{}-{n}.sock", std::process::id()))
    }

    async fn wait_for(path: &Path) {
        for _ in 0..200 {
            if path.exists() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    /// Spawn the real parser thread seeded with `seed`, then the real IPC server bound to `path`.
    fn spawn_real_server(path: &Path, rows: u16, cols: u16, seed: &[u8]) -> IpcServer {
        let (msg_tx, msg_rx) = sync_channel::<Msg>(64);
        screen::spawn_parser_thread(rows, cols, msg_rx);
        msg_tx.send(Msg::Data(seed.to_vec())).unwrap();
        IpcServer::spawn(path, IpcController::new(msg_tx)).unwrap()
    }

    #[tokio::test]
    async fn dump_screen_round_trips_through_real_parser() {
        let path = temp_sock();
        let _server = spawn_real_server(&path, 24, 80, b"hello world");
        wait_for(&path).await;

        let mut conn = IpcClient::new(&path).connect().await.expect("connect");
        let snap = conn.dump_screen().await.expect("dump_screen");

        assert_eq!(snap.col_count(), 80);
        assert_eq!(snap.row_count(), 24);
        assert!(
            snap.formatted_rows().iter().any(|row| row.contains("hello world")),
            "screen did not contain seeded text: {:?}",
            snap.formatted_rows()
        );

        conn.close().await.expect("close");
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn server_serves_sequential_clients_after_drop() {
        let path = temp_sock();
        let _server = spawn_real_server(&path, 10, 40, b"first");
        wait_for(&path).await;

        let mut first = IpcClient::new(&path).connect().await.expect("connect 1");
        let snap1 = first.dump_screen().await.expect("dump 1");
        drop(first); // Drop closes the fd; server should see EOF and serve the next client.

        let mut second = IpcClient::new(&path).connect().await.expect("connect 2");
        let snap2 = second.dump_screen().await.expect("dump 2");

        assert_eq!(snap1.col_count(), snap2.col_count());
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn connect_rejects_version_mismatch() {
        let path = temp_sock();
        let _ = std::fs::remove_file(&path);
        let listener = StdUnixListener::bind(&path).unwrap();

        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut len_bytes = [0u8; wire::LEN_PREFIX_BYTES];
            stream.read_exact(&mut len_bytes).unwrap();
            let len = wire::parse_len(len_bytes).unwrap();
            let mut body = vec![0u8; len];
            stream.read_exact(&mut body).unwrap();
            let _req: Req = wire::decode_body(&body).unwrap();

            let framed = wire::encode_frame(&Rep::Hello(HelloRep {
                version: PROTOCOL_VERSION + 1,
            }))
            .unwrap();
            stream.write_all(&framed).unwrap();
            stream.flush().unwrap();
        });

        let err = IpcClient::new(&path).connect().await.unwrap_err();
        assert!(
            matches!(err, IpcError::ProtocolMismatch { ours, theirs }
                if ours == PROTOCOL_VERSION && theirs == PROTOCOL_VERSION + 1),
            "unexpected error: {err:?}"
        );

        handle.join().unwrap();
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn connect_fails_without_server() {
        let path = temp_sock();
        let err = IpcClient::new(&path).connect().await.unwrap_err();
        assert!(matches!(err, IpcError::Connect { .. }), "unexpected error: {err:?}");
    }
}
