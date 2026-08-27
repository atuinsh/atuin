//! Utility library for interacting with the `pty-proxy` over its native IPC channel.
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

#[cfg(all(test, feature = "server"))]
mod tests {
    use std::io::{Read as _, Write as _};
    use std::os::unix::net::UnixListener as StdUnixListener;
    use std::path::Path;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::mpsc::sync_channel;
    use std::thread::JoinHandle;

    use rstest::{fixture, rstest};

    use super::*;
    use crate::ipc::controller::IpcController;
    use crate::ipc::server::IpcServer;
    use crate::screen::{self, Msg};

    struct TempSock(PathBuf);

    impl TempSock {
        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempSock {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    #[fixture]
    fn sock() -> TempSock {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        TempSock(
            std::env::temp_dir().join(format!("atuin-ipc-test-{}-{n}.sock", std::process::id())),
        )
    }

    fn serve(sock: &Path, rows: u16, cols: u16, seed: &[u8]) {
        let (msg_tx, msg_rx) = sync_channel::<Msg>(64);
        screen::spawn_parser_thread(rows, cols, msg_rx);
        msg_tx.send(Msg::Data(seed.to_vec())).unwrap();
        IpcServer::spawn(sock, IpcController::new(msg_tx)).unwrap();
    }

    fn canned_server(sock: &Path, rep: Rep) -> JoinHandle<()> {
        let listener = StdUnixListener::bind(sock).unwrap();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut len_bytes = [0u8; wire::LEN_PREFIX_BYTES];
            stream.read_exact(&mut len_bytes).unwrap();
            let len = wire::parse_len(len_bytes).unwrap();
            let mut body = vec![0u8; len];
            stream.read_exact(&mut body).unwrap();
            let _: Req = wire::decode_body(&body).unwrap();
            stream.write_all(&wire::encode_frame(&rep).unwrap()).unwrap();
            stream.flush().unwrap();
        })
    }

    #[rstest]
    #[case(24, 80, "hello world")]
    #[case(10, 40, "another line")]
    #[case(1, 200, "single wide row")]
    #[tokio::test]
    async fn dump_screen_reflects_live_screen(
        sock: TempSock,
        #[case] rows: u16,
        #[case] cols: u16,
        #[case] seed: &str,
    ) {
        serve(sock.path(), rows, cols, seed.as_bytes());

        let mut conn = IpcClient::new(sock.path()).connect().await.expect("connect");
        let snap = conn.dump_screen().await.expect("dump_screen");

        assert_eq!((snap.row_count(), snap.col_count()), (rows, cols));
        assert_eq!((snap.cursor_row(), usize::from(snap.cursor_col())), (0, seed.len()));
        assert!(
            snap.formatted_rows().iter().any(|row| row.contains(seed)),
            "screen missing seeded text {seed:?}: {:?}",
            snap.formatted_rows()
        );

        conn.close().await.expect("close");
    }

    #[rstest]
    #[tokio::test]
    async fn drop_lets_server_serve_the_next_client(sock: TempSock) {
        serve(sock.path(), 10, 40, b"screen");

        let mut first = IpcClient::new(sock.path()).connect().await.expect("connect 1");
        first.dump_screen().await.expect("dump 1");
        drop(first);

        let mut second = IpcClient::new(sock.path()).connect().await.expect("connect 2");
        assert_eq!(second.dump_screen().await.expect("dump 2").col_count(), 40);
    }

    #[rstest]
    #[tokio::test]
    async fn connect_rejects_version_mismatch(sock: TempSock) {
        let theirs = PROTOCOL_VERSION + 1;
        let server = canned_server(sock.path(), Rep::Hello(HelloRep { version: theirs }));

        let err = IpcClient::new(sock.path()).connect().await.unwrap_err();

        assert!(
            matches!(err, IpcError::ProtocolMismatch { ours, theirs: got }
                if ours == PROTOCOL_VERSION && got == theirs),
            "unexpected error: {err:?}"
        );
        server.join().unwrap();
    }

    #[rstest]
    #[tokio::test]
    async fn connect_rejects_wrong_reply_variant(sock: TempSock) {
        let reply = Rep::DumpScreenRep(DumpScreenRep { screen: ScreenSnapshot::default() });
        let server = canned_server(sock.path(), reply);

        let err = IpcClient::new(sock.path()).connect().await.unwrap_err();

        assert!(matches!(err, IpcError::UnexpectedReply), "unexpected error: {err:?}");
        server.join().unwrap();
    }

    #[rstest]
    #[tokio::test]
    async fn connect_fails_without_server(sock: TempSock) {
        let err = IpcClient::new(sock.path()).connect().await.unwrap_err();
        assert!(matches!(err, IpcError::Connect { .. }), "unexpected error: {err:?}");
    }
}
