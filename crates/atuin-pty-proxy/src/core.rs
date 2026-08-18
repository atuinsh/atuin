//! Test harness for the socket-serving half of the proxy.
//!
//! The shipped session is [`crate::runtime::start`], which owns raw mode,
//! SIGWINCH, the compositor and stdin — none of which a test process has.
//! This wires the same parser and socket-server threads to an injected PTY
//! and directory so the framed subscriber protocol can be driven for real.
//!
//! Deliberately *not* a second copy of the runtime's output pump: it mirrors
//! bytes and nothing else. Anything richer here (highlighting, capture) would
//! be a parallel implementation that no shipped path runs, and a divergence
//! from it would read as coverage while proving nothing.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::thread::JoinHandle;

use crate::screen::{self, Msg};

/// Endpoints for [`ProxyCore`], injected so a test can supply a real PTY and
/// a tempdir.
pub struct ProxyCoreConfig {
    /// PTY master read side.
    pub reader: Box<dyn Read + Send>,
    /// PTY master write side.
    pub writer: Box<dyn Write + Send>,
    /// Where PTY output is mirrored.
    pub mirror: Box<dyn Write + Send>,
    /// Initial screen rows.
    pub rows: u16,
    /// Initial screen columns.
    pub cols: u16,
    /// Per-proxy directory for the socket and token.
    pub dir: PathBuf,
}

/// The socket-serving half of the proxy, driven without a terminal.
pub struct ProxyCore {
    msg_tx: SyncSender<Msg>,
    input_tx: SyncSender<Vec<u8>>,
    current_cols: Arc<AtomicU16>,
    output_thread: Option<JoinHandle<()>>,
    sock_path: PathBuf,
    token_path: PathBuf,
}

/// A cloneable handle for feeding resizes into the engine.
#[derive(Clone)]
pub struct ProxyHandle {
    msg_tx: SyncSender<Msg>,
    current_cols: Arc<AtomicU16>,
}

impl ProxyHandle {
    /// Record a new terminal size and fan it out to subscribers.
    pub fn resize(&self, rows: u16, cols: u16) {
        self.current_cols.store(cols.max(1), Ordering::Relaxed);
        let _ = self.msg_tx.send(Msg::Resize { rows, cols });
    }
}

impl ProxyCore {
    /// Create the socket directory and token, bind the socket, and start the
    /// engine threads.
    ///
    /// # Errors
    ///
    /// Fails if the directory, token, or socket cannot be created.
    pub fn spawn(config: ProxyCoreConfig) -> eyre::Result<Self> {
        let ProxyCoreConfig {
            reader,
            writer,
            mirror,
            rows,
            cols,
            dir,
        } = config;

        screen::create_proxy_dir(&dir)?;
        let token = screen::write_token(&dir)?;
        let sock_path = screen::socket_path_in(&dir);
        let listener = std::os::unix::net::UnixListener::bind(&sock_path)?;

        let (msg_tx, msg_rx) = mpsc::sync_channel::<Msg>(64);
        let (input_tx, input_rx) = mpsc::sync_channel::<Vec<u8>>(64);
        let current_cols = Arc::new(AtomicU16::new(cols.max(1)));

        screen::spawn_parser_thread(rows, cols, msg_rx);
        screen::spawn_socket_server(listener, msg_tx.clone(), token, input_tx.clone());
        spawn_pty_writer_thread(writer, input_rx);
        let output_thread = spawn_output_pump(reader, mirror, msg_tx.clone());

        Ok(Self {
            msg_tx,
            input_tx,
            current_cols,
            output_thread: Some(output_thread),
            sock_path,
            token_path: screen::token_path_in(&dir),
        })
    }

    /// The bound socket path advertised to clients.
    #[must_use]
    pub fn socket_path(&self) -> &Path {
        &self.sock_path
    }

    /// The input token file next to the socket.
    #[must_use]
    pub fn token_path(&self) -> &Path {
        &self.token_path
    }

    /// Handle for feeding resizes into the engine.
    #[must_use]
    pub fn handle(&self) -> ProxyHandle {
        ProxyHandle {
            msg_tx: self.msg_tx.clone(),
            current_cols: self.current_cols.clone(),
        }
    }

    /// Sender into the PTY writer thread.
    #[must_use]
    pub fn input_sender(&self) -> SyncSender<Vec<u8>> {
        self.input_tx.clone()
    }

    /// Wait for the output pump to finish.
    pub fn join_output(&mut self) {
        if let Some(thread) = self.output_thread.take() {
            let _ = thread.join();
        }
    }
}

/// Feed PTY output to the screen model and the mirror, in that order, and
/// mark the end of the stream — the ordering the subscriber protocol relies
/// on, and all these tests need from a pump.
fn spawn_output_pump(
    mut pty_reader: Box<dyn Read + Send>,
    mut mirror: Box<dyn Write + Send>,
    msg_tx: SyncSender<Msg>,
) -> JoinHandle<()> {
    std::thread::spawn(move || {
        let mut buf = [0u8; 8192];

        loop {
            match pty_reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let _ = msg_tx.send(Msg::Data(buf[..n].to_vec()));
                    if mirror.write_all(&buf[..n]).is_err() {
                        break;
                    }
                    let _ = mirror.flush();
                }
            }
        }

        let _ = msg_tx.send(Msg::Eof);
    })
}

fn spawn_pty_writer_thread(mut writer: Box<dyn Write + Send>, input_rx: Receiver<Vec<u8>>) {
    std::thread::spawn(move || {
        for chunk in input_rx {
            if writer.write_all(&chunk).is_err() {
                break;
            }
            let _ = writer.flush();
        }
    });
}
