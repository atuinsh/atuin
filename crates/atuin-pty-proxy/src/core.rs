use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::thread::JoinHandle;

use crate::CommandCaptureSink;
use crate::capture::CommandCaptureTracker;
use crate::debug::{Osc133DebugHighlighter, RESET};
use crate::screen::{self, Msg};

/// Everything the proxy engine needs, with the endpoints injected so tests
/// can drive a real PTY and socket without touching the terminal.
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
    /// Highlight OSC 133 regions in mirrored output.
    pub debug_osc133: bool,
    /// Optional sink for captured command output.
    pub command_capture_sink: Option<CommandCaptureSink>,
}

/// The embeddable proxy engine used by the share integration and tests.
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
            debug_osc133,
            command_capture_sink,
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
        let output_thread = spawn_output_pump(OutputPump {
            pty_reader: reader,
            mirror,
            msg_tx: msg_tx.clone(),
            debug_osc133,
            command_capture_sink,
            current_cols: current_cols.clone(),
        });

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

struct OutputPump {
    pty_reader: Box<dyn Read + Send>,
    mirror: Box<dyn Write + Send>,
    msg_tx: SyncSender<Msg>,
    debug_osc133: bool,
    command_capture_sink: Option<CommandCaptureSink>,
    current_cols: Arc<AtomicU16>,
}

fn spawn_output_pump(pump: OutputPump) -> JoinHandle<()> {
    std::thread::spawn(move || {
        let OutputPump {
            mut pty_reader,
            mut mirror,
            msg_tx,
            debug_osc133,
            command_capture_sink,
            current_cols,
        } = pump;

        let mut highlighter = debug_osc133.then(Osc133DebugHighlighter::new);
        let mut capture_tracker = command_capture_sink
            .as_ref()
            .map(|_| CommandCaptureTracker::new(current_cols));
        let mut buf = [0u8; 8192];

        loop {
            match pty_reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if let (Some(tracker), Some(sink)) =
                        (capture_tracker.as_mut(), command_capture_sink.as_ref())
                    {
                        tracker.push(&buf[..n], sink);
                    }

                    if let Some(highlighter) = highlighter.as_mut() {
                        let rendered = highlighter.render(&buf[..n]);
                        let _ = msg_tx.send(Msg::Data(rendered.clone()));

                        if mirror.write_all(&rendered).is_err() {
                            break;
                        }
                    } else {
                        let _ = msg_tx.send(Msg::Data(buf[..n].to_vec()));

                        if mirror.write_all(&buf[..n]).is_err() {
                            break;
                        }
                    }
                    let _ = mirror.flush();
                }
            }
        }

        let _ = msg_tx.send(Msg::Eof);

        if highlighter.is_some() {
            let _ = mirror.write_all(RESET);
            let _ = mirror.flush();
        }
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
