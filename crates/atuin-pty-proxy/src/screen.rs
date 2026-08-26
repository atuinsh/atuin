use std::io::Write;
use std::num::NonZeroU16;
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, SyncSender};

use atuin_common::os::unix::{SecureTempDirError, create_secure_temp_dir};

pub enum Msg {
    Data(Vec<u8>),
    Resize {
        rows: u16,
        cols: u16,
    },
    ScreenRequest(mpsc::Sender<Vec<u8>>),
}

pub fn socket_path() -> Result<PathBuf, SecureTempDirError> {
    let uid = atuin_common::os::unix::uid();
    let dir = atuin_common::os::unix::tmp_dir().join(format!("atuin-{uid}"));
    let dir = create_secure_temp_dir(dir)?;
    Ok(dir.join(format!("pty-proxy-{}.sock", std::process::id())))
}

pub fn spawn_parser_thread(rows: u16, cols: u16, msg_rx: Receiver<Msg>) {
    std::thread::spawn(move || {
        let rows = NonZeroU16::new(rows).unwrap_or(NonZeroU16::MIN);
        let cols = NonZeroU16::new(cols).unwrap_or(NonZeroU16::MIN);
        let mut parser = vt100::Parser::new(rows, cols, 0);

        loop {
            let Ok(first) = msg_rx.recv() else {
                break;
            };

            handle_parser_msg(&mut parser, first);

            while let Ok(msg) = msg_rx.try_recv() {
                handle_parser_msg(&mut parser, msg);
            }
        }
    });
}

pub fn spawn_socket_server(sock_path: PathBuf, screen_tx: SyncSender<Msg>) {
    std::thread::spawn(move || {
        let listener = match UnixListener::bind(&sock_path) {
            Ok(l) => l,
            Err(e) => {
                eprintln!("atuin pty-proxy: failed to bind socket: {e}");
                return;
            }
        };

        for stream in listener.incoming() {
            let Ok(mut stream) = stream else {
                break;
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

    let mut buf = Vec::with_capacity(256 + (usize::from(rows) * usize::from(cols)));
    buf.extend_from_slice(&rows.to_be_bytes());
    buf.extend_from_slice(&cols.to_be_bytes());
    buf.extend_from_slice(&cursor_row.to_be_bytes());
    buf.extend_from_slice(&cursor_col.to_be_bytes());

    for row_data in screen.rows_formatted(0, cols) {
        let len = row_data.len() as u32;
        buf.extend_from_slice(&len.to_be_bytes());
        buf.extend_from_slice(row_data.as_bytes());
    }

    buf
}

fn handle_parser_msg(parser: &mut vt100::Parser, msg: Msg) {
    match msg {
        Msg::Data(data) => parser.process(&data),
        Msg::Resize { rows, cols } => {
            let rows = NonZeroU16::new(rows).unwrap_or(NonZeroU16::MIN);
            let cols = NonZeroU16::new(cols).unwrap_or(NonZeroU16::MIN);
            parser.screen_mut().set_size(rows, cols);
        }
        Msg::ScreenRequest(reply_tx) => {
            let _ = reply_tx.send(encode_screen(parser));
        }
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    /// Get the `rows` and `cols` values from an [`encode_screen`] blob.
    fn size_of(blob: &[u8]) -> (u16, u16) {
        (u16::from_be_bytes([blob[0], blob[1]]), u16::from_be_bytes([blob[2], blob[3]]))
    }

    #[rstest]
    fn init_small_and_wrap(#[values(0, 1, 2, 3)] rows: u16, #[values(0, 1, 2, 3)] cols: u16) {
        let (msg_tx, msg_rx) = mpsc::sync_channel(8);
        spawn_parser_thread(rows, cols, msg_rx);
        msg_tx.send(Msg::Data(b"hello world".to_vec())).expect("parser thread alive");

        let (reply_tx, reply_rx) = mpsc::channel();
        msg_tx.send(Msg::ScreenRequest(reply_tx)).expect("parser thread alive");
        let blob = reply_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("parser thread still answering");

        // Dimensions are clamped to (1, 1) because vt100 dimensions must be positive.
        assert_eq!(size_of(&blob), (rows.max(1), cols.max(1)));
    }

    #[rstest]
    fn resize_small_and_wrap(#[values(0, 1, 2, 3)] rows: u16, #[values(0, 1, 2, 3)] cols: u16) {
        let mut parser = vt100::Parser::default();
        handle_parser_msg(&mut parser, Msg::Resize { rows, cols });
        handle_parser_msg(&mut parser, Msg::Data(b"hello world".to_vec()));

        // Dimensions are clamped to (1, 1) because vt100 dimensions must be positive.
        assert_eq!(size_of(&encode_screen(&parser)), (rows.max(1), cols.max(1)));
    }
}
