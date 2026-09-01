use std::io::{Read as _, Write as _};
use std::os::unix::net::UnixListener as StdUnixListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::sync_channel;
use std::thread::JoinHandle;

use rstest::{fixture, rstest};

use crate::ipc::domain::{
    AnyRequest, AnyResponse, DumpScreenResponse, HelloResponse, PROTOCOL_VERSION,
};
use crate::ipc::{IpcClient, IpcConnectError, IpcController, IpcError, IpcServer, wire};
use crate::screen::{self, Msg, ScreenSnapshot};

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
    TempSock(std::env::temp_dir().join(format!("atuin-ipc-test-{}-{n}.sock", std::process::id())))
}

fn serve(sock: &Path, rows: u16, cols: u16, seed: &[u8]) {
    let (msg_tx, msg_rx) = sync_channel::<Msg>(64);
    screen::spawn_parser_thread(
        rows,
        cols,
        msg_rx,
        screen::ParserOptions {
            sink: None,
            debug_osc133: false,
        },
    );
    msg_tx.send(Msg::Data(seed.to_vec())).unwrap();
    IpcServer::spawn(sock, IpcController::new(msg_tx)).unwrap();
}

fn canned_server(sock: &Path, rep: AnyResponse) -> JoinHandle<()> {
    let listener = StdUnixListener::bind(sock).unwrap();
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut header_bytes = [0u8; wire::Header::SERIALIZED_LEN];
        stream.read_exact(&mut header_bytes).unwrap();
        let header = wire::Header::parse(header_bytes).unwrap();
        let body_len = header.message_width as usize - wire::Header::SERIALIZED_LEN;
        let mut body = vec![0u8; body_len];
        stream.read_exact(&mut body).unwrap();
        let _: AnyRequest = wire::decode_body(&body).unwrap();
        stream.write_all(&wire::try_encode(&rep).unwrap()).unwrap();
        stream.flush().unwrap();
    })
}

/// A legacy (v18.20.1) proxy: on connect, push the raw screen dump and close.
fn v0_server(
    sock: &Path,
    dims: (u16, u16),
    cursor: (u16, u16),
    lines: Vec<String>,
) -> JoinHandle<()> {
    let listener = StdUnixListener::bind(sock).unwrap();
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = Vec::new();
        buf.extend_from_slice(&dims.0.to_be_bytes());
        buf.extend_from_slice(&dims.1.to_be_bytes());
        buf.extend_from_slice(&cursor.0.to_be_bytes());
        buf.extend_from_slice(&cursor.1.to_be_bytes());
        for line in &lines {
            buf.extend_from_slice(&u32::try_from(line.len()).unwrap().to_be_bytes());
            buf.extend_from_slice(line.as_bytes());
        }
        stream.write_all(&buf).unwrap();
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

    let mut conn = IpcClient::new(sock.path())
        .with_protocol(Some(PROTOCOL_VERSION))
        .connect()
        .await
        .expect("connect");
    let snap = conn.dump_screen().await.expect("dump_screen");

    assert_eq!((snap.row_count(), snap.col_count()), (rows, cols));
    assert_eq!((snap.cursor_row(), usize::from(snap.cursor_col())), (0, seed.len()));
    assert!(
        snap.rows.iter().any(|row| row.contains(seed)),
        "screen missing seeded text {seed:?}: {:?}",
        snap.rows
    );

    conn.close().await.expect("close");
}

#[rstest]
#[tokio::test]
async fn drop_lets_server_serve_the_next_client(sock: TempSock) {
    serve(sock.path(), 10, 40, b"screen");

    let mut first = IpcClient::new(sock.path())
        .with_protocol(Some(PROTOCOL_VERSION))
        .connect()
        .await
        .expect("connect 1");
    first.dump_screen().await.expect("dump 1");
    drop(first);

    let mut second = IpcClient::new(sock.path())
        .with_protocol(Some(PROTOCOL_VERSION))
        .connect()
        .await
        .expect("connect 2");
    assert_eq!(second.dump_screen().await.expect("dump 2").col_count(), 40);
}

#[rstest]
#[tokio::test]
async fn connect_rejects_version_mismatch(sock: TempSock) {
    let theirs = PROTOCOL_VERSION + 1;
    let server = canned_server(sock.path(), AnyResponse::Hello(HelloResponse { version: theirs }));

    let err = IpcClient::new(sock.path())
        .with_protocol(Some(PROTOCOL_VERSION))
        .connect()
        .await
        .unwrap_err();

    assert!(
        matches!(err, IpcConnectError::ProtocolMismatch { ours, theirs: got }
            if ours == PROTOCOL_VERSION && got == theirs),
        "unexpected error: {err:?}"
    );
    server.join().unwrap();
}

#[rstest]
#[tokio::test]
async fn connect_rejects_wrong_reply_variant(sock: TempSock) {
    let reply = AnyResponse::DumpScreenResponse(DumpScreenResponse {
        screen: ScreenSnapshot::default(),
    });
    let server = canned_server(sock.path(), reply);

    let err = IpcClient::new(sock.path())
        .with_protocol(Some(PROTOCOL_VERSION))
        .connect()
        .await
        .unwrap_err();

    assert!(
        matches!(err, IpcConnectError::Handshake(IpcError::UnexpectedReply)),
        "unexpected error: {err:?}"
    );
    server.join().unwrap();
}

#[rstest]
#[tokio::test]
async fn connect_fails_without_server(sock: TempSock) {
    let err = IpcClient::new(sock.path())
        .with_protocol(Some(PROTOCOL_VERSION))
        .connect()
        .await
        .unwrap_err();
    assert!(matches!(err, IpcConnectError::Connect { .. }), "unexpected error: {err:?}");
}

#[rstest]
#[tokio::test]
async fn v0_client_reads_legacy_push(sock: TempSock) {
    let server = v0_server(sock.path(), (10, 40), (2, 5), vec!["hello".into(), "world".into()]);

    // No advertised protocol => the client speaks the legacy V0 push protocol.
    let mut conn = IpcClient::new(sock.path()).connect().await.expect("connect");
    let snap = conn.dump_screen().await.expect("dump_screen");

    assert_eq!((snap.row_count(), snap.col_count()), (10, 40));
    assert_eq!((snap.cursor_row(), snap.cursor_col()), (2, 5));
    let got: Vec<&str> = snap.rows.iter().map(String::as_str).collect();
    assert_eq!(got, ["hello", "world"]);

    server.join().unwrap();
}
