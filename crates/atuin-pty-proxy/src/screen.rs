#[cfg(feature = "server")]
use std::num::NonZeroU16;
#[cfg(feature = "server")]
use std::path::PathBuf;
#[cfg(feature = "server")]
use std::sync::mpsc::{self, Receiver};

#[cfg(feature = "server")]
use atuin_common::os::unix::{SecureTempDirError, create_secure_temp_dir};
use serde::{Deserialize, Serialize};

#[cfg(feature = "server")]
pub enum Msg {
    Data(Vec<u8>),
    Resize {
        rows: u16,
        cols: u16,
    },
    ScreenRequest(mpsc::Sender<ScreenSnapshot>),
}

#[cfg(feature = "server")]
pub fn socket_path() -> Result<PathBuf, SecureTempDirError> {
    let uid = atuin_common::os::unix::uid();
    let dir = atuin_common::os::unix::tmp_dir().join(format!("atuin-{uid}"));
    let dir = create_secure_temp_dir(dir)?;
    Ok(dir.join(format!("pty-proxy-{}.sock", std::process::id())))
}

#[cfg(feature = "server")]
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

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ScreenSnapshot {
    screen_dims: (u16, u16),
    cursor_pos: (u16, u16),
    rows: Vec<String>,
}

impl ScreenSnapshot {
    #[must_use]
    pub fn row_count(&self) -> u16 {
        self.screen_dims.0
    }

    #[must_use]
    pub fn col_count(&self) -> u16 {
        self.screen_dims.1
    }

    #[must_use]
    pub fn cursor_row(&self) -> u16 {
        self.cursor_pos.0
    }

    #[must_use]
    pub fn cursor_col(&self) -> u16 {
        self.cursor_pos.1
    }

    #[must_use]
    pub fn formatted_rows(&self) -> &[String] {
        &self.rows
    }
}

#[cfg(feature = "server")]
fn handle_parser_msg(parser: &mut vt100::Parser, msg: Msg) {
    match msg {
        Msg::Data(data) => parser.process(&data),
        Msg::Resize { rows, cols } => {
            let rows = NonZeroU16::new(rows).unwrap_or(NonZeroU16::MIN);
            let cols = NonZeroU16::new(cols).unwrap_or(NonZeroU16::MIN);
            parser.screen_mut().set_size(rows, cols);
        }
        Msg::ScreenRequest(reply_tx) => {
            let screen = parser.screen();
            let _ = reply_tx.send(ScreenSnapshot {
                screen_dims: screen.size(),
                cursor_pos: screen.cursor_position(),
                rows: screen.rows_formatted(0, screen.size().1).collect(),
            });
        }
    }
}
