use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::thread::JoinHandle;

use crossterm::terminal;
use portable_pty::{CommandBuilder, PtySize, native_pty_system};

use crate::CommandCaptureSink;
use crate::capture::CommandCaptureTracker;
use crate::debug::{Osc133DebugHighlighter, RESET};
use crate::pty_proxy::RuntimeOptions;
use crate::screen::{self, Msg};

pub fn main(options: RuntimeOptions) {
    if let Err(e) = run(options) {
        let _ = terminal::disable_raw_mode();
        eprintln!("atuin pty-proxy: {e:#}");
        std::process::exit(1);
    }
}

fn run(options: RuntimeOptions) -> eyre::Result<()> {
    let (cols, rows) = terminal::size()?;

    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| eyre::eyre!("{e:#}"))?;

    // The socket lives in a per-proxy 0700 directory next to its input
    // token. Validate the sockaddr_un length limit BEFORE exporting the env
    // var, falling back to a literal /tmp path, so we never advertise a
    // socket we cannot bind.
    let mut dir = screen::default_proxy_dir();
    if !screen::socket_path_fits(&screen::socket_path_in(&dir)) {
        dir = screen::fallback_proxy_dir();
    }
    let sock_path = screen::socket_path_in(&dir);

    let mut cmd = match options.shell {
        Some(ref path) => CommandBuilder::new(path),
        None => CommandBuilder::new_default_prog(),
    };
    cmd.cwd(std::env::current_dir()?);
    // Reflect the shell we actually spawn in `$SHELL` so the child — and
    // anything it execs via `$SHELL -c` (e.g. fzf's `become`) — sees the
    // shell the user asked for instead of a stale value inherited from the
    // parent environment.
    if let Some(ref path) = options.shell {
        cmd.env("SHELL", path);
    }
    cmd.env("ATUIN_PTY_PROXY_SOCKET", sock_path.as_os_str());
    cmd.env("ATUIN_PTY_PROXY_ACTIVE", "1");
    // Atuin sets a restrictive process-wide umask on startup to protect the
    // files it creates. The shell must not inherit it (#3695) — restore the
    // umask the user launched us with. Applied in the child between fork and
    // exec, so the proxy's own umask stays restrictive.
    if let Some(mask) = options.child_umask {
        cmd.umask(Some(mask as _));
    }

    let mut child = pair.slave.spawn_command(cmd).map_err(|e| eyre::eyre!("{e:#}"))?;

    drop(pair.slave);

    let pty_reader = pair.master.try_clone_reader().map_err(|e| eyre::eyre!("{e:#}"))?;
    let pty_writer = pair.master.take_writer().map_err(|e| eyre::eyre!("{e:#}"))?;

    let mut core = ProxyCore::spawn(ProxyCoreConfig {
        reader: pty_reader,
        writer: pty_writer,
        mirror: Box::new(std::io::stdout()),
        rows,
        cols,
        dir: dir.clone(),
        debug_osc133: options.debug_osc133,
        command_capture_sink: options.command_capture_sink,
    })?;

    spawn_resize_handler(pair.master, core.handle())?;
    spawn_stdin_pump(core.input_sender());

    terminal::enable_raw_mode()?;

    let status = child.wait()?;
    core.join_output();

    let _ = terminal::disable_raw_mode();
    let _ = std::fs::remove_dir_all(&dir);

    std::process::exit(process_exit_code(status.exit_code()));
}

/// Everything the proxy engine needs, with the endpoints injected so tests
/// can drive a real PTY and a real socket without touching the terminal
/// (raw mode, SIGWINCH, and stdin stay in `run`).
pub struct ProxyCoreConfig {
    /// PTY master read side (the shell's output).
    pub reader: Box<dyn Read + Send>,
    /// PTY master write side (the shell's input).
    pub writer: Box<dyn Write + Send>,
    /// Where PTY output is mirrored — the user's terminal in production,
    /// `std::io::sink()` in tests.
    pub mirror: Box<dyn Write + Send>,
    /// Initial screen rows.
    pub rows: u16,
    /// Initial screen columns.
    pub cols: u16,
    /// Per-proxy directory for the socket and token. Owned by the proxy: it
    /// is wiped and recreated with mode 0700 on spawn.
    pub dir: PathBuf,
    /// Highlight OSC 133 regions in the mirrored output.
    pub debug_osc133: bool,
    /// Optional sink for captured command output.
    pub command_capture_sink: Option<CommandCaptureSink>,
}

/// The proxy engine: parser thread, socket server, pty-writer thread, and
/// output pump, running over the injected endpoints.
pub struct ProxyCore {
    msg_tx: SyncSender<Msg>,
    input_tx: SyncSender<Vec<u8>>,
    current_cols: Arc<AtomicU16>,
    output_thread: Option<JoinHandle<()>>,
    sock_path: PathBuf,
    token_path: PathBuf,
}

/// A cloneable handle for feeding resizes into the engine (the SIGWINCH
/// thread in production, the test harness in tests).
#[derive(Clone)]
pub struct ProxyHandle {
    msg_tx: SyncSender<Msg>,
    current_cols: Arc<AtomicU16>,
}

impl ProxyHandle {
    /// Record a new terminal size: updates the column tracker and feeds the
    /// parser (which fans a Resize frame out to subscribers). Lossless
    /// blocking send — see the invariant on `screen::spawn_parser_thread`.
    pub fn resize(&self, rows: u16, cols: u16) {
        self.current_cols.store(cols.max(1), Ordering::Relaxed);
        let _ = self.msg_tx.send(Msg::Resize { rows, cols });
    }
}

impl ProxyCore {
    /// Create the socket directory and token, bind the socket, and start
    /// the engine threads. The listener is bound before this returns, so a
    /// caller may connect immediately.
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

    /// The bound socket path (advertised via `ATUIN_PTY_PROXY_SOCKET`).
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

    /// Sender into the pty-writer thread (the stdin pump's destination).
    /// Sends are lossless (blocking) by design.
    #[must_use]
    pub fn input_sender(&self) -> SyncSender<Vec<u8>> {
        self.input_tx.clone()
    }

    /// Wait for the output pump to finish (PTY EOF). After this, End has
    /// been queued to every subscriber.
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

/// The output pump: PTY bytes -> capture tracker -> parser (lossless) ->
/// mirror. Sends `Msg::Eof` when the PTY read loop exits so the parser can
/// broadcast End to subscribers.
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
        let mut capture_tracker =
            command_capture_sink.as_ref().map(|_| CommandCaptureTracker::new(current_cols));
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
                        // Lossless: blocking send so subscribers never see a
                        // silently diverged screen. The parser never blocks
                        // (see `screen::spawn_parser_thread`), so this can
                        // only briefly backpressure this pump.
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

        // The PTY read loop is done (shell exit, terminal close, or a
        // mirror failure): let the parser broadcast End to subscribers.
        let _ = msg_tx.send(Msg::Eof);

        if highlighter.is_some() {
            let _ = mirror.write_all(RESET);
            let _ = mirror.flush();
        }
    })
}

/// The single owner of the PTY write side, draining input chunks from the
/// stdin pump and from authenticated socket subscribers.
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

fn spawn_stdin_pump(input_tx: SyncSender<Vec<u8>>) {
    std::thread::spawn(move || {
        let mut stdin = std::io::stdin();
        let mut buf = [0u8; 8192];
        loop {
            match stdin.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    // Lossless: user keystrokes must never be dropped. The
                    // pty-writer thread drains this queue as fast as the
                    // shell accepts bytes.
                    if input_tx.send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
            }
        }
    });
}

fn spawn_resize_handler(
    master: Box<dyn portable_pty::MasterPty + Send>,
    handle: ProxyHandle,
) -> eyre::Result<()> {
    use signal_hook::consts::SIGWINCH;
    use signal_hook::iterator::Signals;

    let mut signals = Signals::new([SIGWINCH])?;

    std::thread::spawn(move || {
        for _ in signals.forever() {
            if let Ok((cols, rows)) = terminal::size() {
                let _ = master.resize(PtySize {
                    rows,
                    cols,
                    pixel_width: 0,
                    pixel_height: 0,
                });
                handle.resize(rows, cols);
            }
        }
    });

    Ok(())
}

fn process_exit_code(code: u32) -> i32 {
    i32::try_from(code).unwrap_or(1)
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::process_exit_code;

    #[rstest]
    #[case::zero(0, 0)]
    #[case::mid_range(127, 127)]
    #[case::max_i32(i32::MAX as u32, i32::MAX)]
    #[case::overflow_defaults_to_one(i32::MAX as u32 + 1, 1)]
    fn maps_exit_code(#[case] input: u32, #[case] expected: i32) {
        assert_eq!(process_exit_code(input), expected);
    }
}
