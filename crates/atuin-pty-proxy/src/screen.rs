use std::io::Write;
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, SyncSender};

pub(crate) enum Msg {
    Data(Vec<u8>),
    Resize { rows: u16, cols: u16 },
    ScreenRequest(mpsc::Sender<Vec<u8>>),
}

pub(crate) fn socket_path() -> PathBuf {
    let dir = std::env::temp_dir();
    dir.join(format!("atuin-pty-proxy-{}.sock", std::process::id()))
}

pub(crate) fn spawn_parser_thread(rows: u16, cols: u16, msg_rx: Receiver<Msg>) {
    std::thread::spawn(move || {
        let mut parser = vt100::Parser::new(rows, cols, 0);

        loop {
            let first = match msg_rx.recv() {
                Ok(msg) => msg,
                Err(_) => break,
            };

            handle_parser_msg(&mut parser, first);

            while let Ok(msg) = msg_rx.try_recv() {
                handle_parser_msg(&mut parser, msg);
            }
        }
    });
}

pub(crate) fn spawn_socket_server(sock_path: PathBuf, screen_tx: SyncSender<Msg>) {
    std::thread::spawn(move || {
        let listener = match UnixListener::bind(&sock_path) {
            Ok(l) => l,
            Err(e) => {
                eprintln!("atuin pty-proxy: failed to bind socket: {e}");
                return;
            }
        };

        for stream in listener.incoming() {
            let mut stream = match stream {
                Ok(s) => s,
                Err(_) => break,
            };

            let (reply_tx, reply_rx) = mpsc::channel();
            if screen_tx.send(Msg::ScreenRequest(reply_tx)).is_err() {
                break;
            }
            if let Ok(data) = reply_rx.recv() {
                let _ = stream.write_all(&data);
                let _ = stream.flush();
            }
        }
    });
}

/// Wire format written to the Unix socket:
///
/// ```text
/// [rows: u16 BE][cols: u16 BE][cursor_row: u16 BE][cursor_col: u16 BE]
/// [row_0_len: u32 BE][row_0_bytes...]
/// [row_1_len: u32 BE][row_1_bytes...]
/// ...
/// ```
///
/// Each row's bytes come from `screen.rows_formatted(0, cols)` and contain
/// pre-built ANSI escape sequences. The client can write them directly to
/// stdout without needing its own vt100 parser.
fn encode_screen(parser: &vt100::Parser) -> Vec<u8> {
    let screen = parser.screen();
    let (rows, cols) = screen.size();
    let (cursor_row, cursor_col) = screen.cursor_position();

    let mut buf: Vec<u8> = Vec::with_capacity(256 + (rows as usize * cols as usize));
    buf.extend_from_slice(&rows.to_be_bytes());
    buf.extend_from_slice(&cols.to_be_bytes());
    buf.extend_from_slice(&cursor_row.to_be_bytes());
    buf.extend_from_slice(&cursor_col.to_be_bytes());

    for row_bytes in screen.rows_formatted(0, cols) {
        let len = row_bytes.len() as u32;
        buf.extend_from_slice(&len.to_be_bytes());
        buf.extend_from_slice(&row_bytes);
    }

    buf
}

fn handle_parser_msg(parser: &mut vt100::Parser, msg: Msg) {
    match msg {
        Msg::Data(data) => parser.process(&data),
        Msg::Resize { rows, cols } => parser.screen_mut().set_size(rows, cols),
        Msg::ScreenRequest(reply_tx) => {
            let _ = reply_tx.send(encode_screen(parser));
        }
    }
}
