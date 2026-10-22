use std::os::fd::{AsFd as _, OwnedFd};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use atuin_common::os::unix::io::{ReadRetryingExt as _, WriteAllExt as _};
use atuin_common::os::unix::tty::{Access, AsyncTty, TtyId};
use crossterm::terminal;
use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};
use rustix::termios::SpecialCodeIndex;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::{mpsc, watch};

use crate::debug::RESET;
use crate::foreground::{ForegroundUpdater, pty_parent_fd};
use crate::parser::Parser;
use crate::pty_proxy::RuntimeOptions;
use crate::screen::{self, Msg, SocketServer};
use crate::title::TerminalTitle;

/// The current version of the pty-proxy protocol.
///
/// This enables the shell integration to be compatible with older pty-proxy versions.
///
/// # Version history
///
/// * 1: Initial version. Expects OSC 133 escape sequences.
/// * 2: Switched from OSC 133 to OSC 18188735.
const PROTOCOL_VERSION: &str = "2";

/// How often to update the socket's modification time so it doesn't get automatically deleted by
/// temporary file cleaners.
const SOCKET_KEEPALIVE_INTERVAL: Duration = Duration::from_mins(1);

pub fn main(options: RuntimeOptions) {
    // Everything but the emulator waits on I/O, so it all runs on this one thread: a multi-threaded
    // scheduler would start a worker per core in a process that sits under every interactive shell.
    // Only the I/O and timer drivers are enabled, and the blocking pool starts no thread unless a
    // terminal cannot be reopened.
    let result = tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .enable_time()
        .build()
        .map_err(Error::from)
        .and_then(|runtime| {
            let result = runtime.block_on(run(options));
            // An ordinary drop waits for blocking tasks, and a blocking read of stdin never returns.
            runtime.shutdown_background();
            result
        });

    match result {
        Ok(code) => std::process::exit(code),
        Err(e) => {
            let _ = terminal::disable_raw_mode();
            eprintln!("atuin pty-proxy: {e:#}");
            std::process::exit(1);
        }
    }
}

/// An error that occurred while setting up the PTY server.
#[derive(Debug, thiserror::Error)]
enum InitError {
    #[error("failed to get path to terminal")]
    GetPath,
    #[error("failed to open terminal: {0}")]
    Open(std::io::Error),
    #[error("file is not a terminal")]
    NotATerminal,
    #[error("failed to create socket: {0}")]
    CreateSocket(atuin_common::os::unix::SecureTempDirError),
    #[error("failed to bind socket: {0}")]
    BindSocket(std::io::Error),
}

/// Set the appropriate proxy-related environment variables in the PTY child.
///
/// This sets `ATUIN_PTY_PROXY_SOCKET` to the socket path, or, if `socket_path` is [`None`] (because
/// the server failed to initialize), this sets `ATUIN_PTY_PROXY_FAILED` to stop the child shell
/// from endlessly trying to spawn additional PTY proxies.
fn set_child_env(cmd: &mut CommandBuilder, socket_path: Option<&std::path::Path>) {
    cmd.env("ATUIN_PTY_PROXY_ACTIVE", PROTOCOL_VERSION);

    if let Some(path) = socket_path {
        cmd.env("ATUIN_PTY_PROXY_SOCKET", path);
        cmd.env_remove("ATUIN_PTY_PROXY_FAILED");
    } else {
        cmd.env_remove("ATUIN_PTY_PROXY_SOCKET");
        cmd.env("ATUIN_PTY_PROXY_FAILED", "1");
    }
}

#[derive(Debug, thiserror::Error)]
enum Error {
    #[error("failed to open pty: {0}")]
    OpenPty(Box<dyn std::error::Error + Send + Sync + 'static>),
    #[error("failed to spawn child process: {0}")]
    SpawnCommand(Box<dyn std::error::Error + Send + Sync + 'static>),
    #[error("failed to obtain a descriptor for the pty")]
    MasterFd,
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

async fn run(options: RuntimeOptions) -> Result<i32, Error> {
    let (cols, rows) = terminal::size()?;
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| Error::OpenPty(e.into()))?;

    // The PTY proxy server and the path of its socket.
    let server_and_path: Option<(SocketServer, atuin_common::fs::RemoveOnDropPath)> = pair
        .master
        .tty_name()
        .ok_or(InitError::GetPath)
        .and_then(|tty_name| {
            // portable-pty doesn't give us access to the child FD, so we have to open it by path.
            let tty_fd = std::fs::File::open(&tty_name).map_err(InitError::Open)?;
            let tty_id = TtyId::from_fd(tty_fd).ok_or(InitError::NotATerminal)?;
            let socket_path = screen::socket_path(tty_id).map_err(InitError::CreateSocket)?;

            // A previous proxy on this PTY may have died without cleaning up. Its socket is
            // definitely no longer in use (otherwise we wouldn't have been given its PTY), so
            // delete it.
            let _ = std::fs::remove_file(&socket_path);
            let server = SocketServer::new(&socket_path).map_err(InitError::BindSocket)?;
            let path = atuin_common::fs::RemoveOnDropPath(socket_path);
            Ok::<_, InitError>((server, path))
        })
        .map_err(|e| {
            // If we're unable to initialize the PTY server (e.g., failed to create the socket),
            // print the error and continue rather than returning it, so the user still gets a
            // shell.
            eprintln!("atuin pty-proxy: failed to initialize server: {e}");
        })
        .ok();

    let (msg_tx, msg_rx) = mpsc::channel::<Msg>(64);
    let _emulator_handle = screen::spawn_emulator(rows, cols, msg_rx, screen::EmulatorOptions {
        command_capture: (options.command_capture)(),
        debug_markers: options.debug_markers,
    })?;

    let socket_path = if let Some((server, path)) = server_and_path {
        tokio::spawn(server.serve(msg_tx.clone()));
        spawn_socket_updater(path.to_path_buf());
        Some(path)
    } else {
        None
    };

    let shell = options.shell.first().cloned();
    let mut cmd = CommandBuilder::from_argv(options.shell);
    cmd.cwd(std::env::current_dir()?);
    // Reflect the shell we actually spawn in `$SHELL` so the child — and
    // anything it execs via `$SHELL -c` (e.g. fzf's `become`) — sees the
    // shell the user asked for instead of a stale value inherited from the
    // parent environment.
    if let Some(shell) = shell {
        cmd.env("SHELL", shell);
    }
    set_child_env(&mut cmd, socket_path.as_deref());
    // Atuin sets a restrictive process-wide umask on startup to protect the
    // files it creates. The shell must not inherit it (#3695) — restore the
    // umask the user launched us with. Applied in the child between fork and
    // exec, so the proxy's own umask stays restrictive.
    if let Some(mask) = options.child_umask {
        #[allow(
            clippy::cast_possible_truncation,
            reason = "mode_t is u16 on macOS and u32 on Linux. nop on linux, narrowing on macOS."
        )]
        cmd.umask(Some(mask as _));
    }

    let mut child = pair.slave.spawn_command(cmd).map_err(|e| Error::SpawnCommand(e.into()))?;
    drop(pair.slave);

    let master = pty_parent_fd(pair.master.as_ref()).ok_or(Error::MasterFd)?;
    let mut pty_reader = AsyncTty::new(master.try_clone()?)?;
    let mut pty_writer = AsyncTty::new(master)?;

    // A terminal we cannot open afresh (one owned by another user after `su`, say) is read and
    // written on tokio's blocking threads instead.
    let mut stdin = match AsyncTty::reopen(rustix::stdio::stdin(), Access::Read) {
        Ok(tty) => Stdin::Reopened(tty),
        Err(_) => Stdin::Inherited(Arc::new(std::io::stdin().as_fd().try_clone_to_owned()?)),
    };
    let mut stdout = match AsyncTty::reopen(rustix::stdio::stdout(), Access::Write) {
        Ok(tty) => Stdout::Reopened(tty),
        Err(_) => Stdout::Inherited(Arc::new(std::io::stdout().as_fd().try_clone_to_owned()?)),
    };

    let child_pid =
        child.process_id().and_then(|pid| rustix::process::Pid::from_raw(pid.cast_signed()));
    // Update this process's CWD to match the child's CWD. This is necessary to enable programs like
    // tmux to track the child's CWD. Terminals that name the program running in them would
    // otherwise name the proxy, so the updater also names the child's foreground for the title.
    let (leaders_tx, mut leaders) = watch::channel(None);
    let foreground_updater = ForegroundUpdater::new(pair.master.as_ref(), child_pid, leaders_tx);
    let mut title = TerminalTitle::new();
    let mut sigchld = signal(SignalKind::child())?;
    spawn_resize_handler(pair.master, msg_tx.clone())?;
    terminal::enable_raw_mode()?;

    let debug_markers = options.debug_markers;
    let output = tokio::spawn(async move {
        // The one parser of the child's output. Everything that follows the output takes what it
        // found from here.
        let mut parser = Parser::new();
        let mut buf = [0u8; 8192];

        loop {
            tokio::select! {
                read = pty_reader.read(&mut buf) => match read {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let output = parser.read(buf[..n].to_vec());
                        let labelled = debug_markers.then(|| output.with_marker_labels());
                        if stdout.write_all(labelled.as_deref().unwrap_or(output.data())).await.is_err() {
                            break;
                        }
                        // The labels sit between escape codes, so the terminal ends up where
                        // the output does.
                        title.track(&output);

                        // Only after the terminal has the chunk: handing it to the emulator wakes
                        // the emulator's thread, a syscall that would otherwise delay every echoed
                        // keystroke.
                        let _ = msg_tx.send(Msg::Data(output)).await;
                        foreground_updater.update();
                    }
                },
                Ok(()) = leaders.changed() => {
                    let sequence = leaders
                        .borrow_and_update()
                        .as_ref()
                        .and_then(|leader| title.retitle(leader));
                    if let Some(sequence) = sequence
                        && stdout.write_all(&sequence).await.is_err()
                    {
                        break;
                    }
                }
            }
        }

        if debug_markers {
            let _ = stdout.write_all(RESET).await;
        }
    });

    tokio::spawn(async move {
        let mut buf = [0u8; 8192];
        loop {
            match stdin.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if pty_writer.write_all(&buf[..n]).await.is_err() {
                        break;
                    }
                }
            }
        }
        // Hand the end of our input on to the shell, so that piped input ends the session. EOF
        // only counts at the start of a line, hence the newline.
        if let Ok(termios) = rustix::termios::tcgetattr(&pty_writer)
            && let eof = termios.special_codes[SpecialCodeIndex::VEOF]
            && eof != 0
        {
            let _ = pty_writer.write_all(&[b'\n', eof]).await;
        }
    });

    // Checked once before the first wait, since the child may have exited before the handler was
    // installed.
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        sigchld.recv().await;
    };
    let _ = output.await;

    let _ = terminal::disable_raw_mode();
    drop(socket_path); // delete the socket
    Ok(process_exit_code(status.exit_code()))
}

/// Where the user's input comes from.
enum Stdin {
    /// The user's terminal, opened afresh and driven by the reactor.
    Reopened(AsyncTty),
    /// The inherited stdin, read on the blocking pool.
    ///
    /// Whatever else shares its description may have made it non-blocking, so having nothing to
    /// read yet is waited out rather than taken for the end of input.
    Inherited(Arc<OwnedFd>),
}

impl Stdin {
    /// Read some input into `buf`; `0` means the end of input.
    async fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Self::Reopened(tty) => tty.read(buf).await,
            Self::Inherited(fd) => {
                let (fd, len) = (Arc::clone(fd), buf.len());
                let chunk = tokio::task::spawn_blocking(move || {
                    let mut chunk = vec![0; len];
                    let read = fd.read_retrying(&mut chunk)?;
                    chunk.truncate(read);
                    Ok::<_, rustix::io::Errno>(chunk)
                })
                .await
                .map_err(std::io::Error::other)??;
                buf[..chunk.len()].copy_from_slice(&chunk);
                Ok(chunk.len())
            }
        }
    }
}

/// Where the child's output goes.
enum Stdout {
    /// The user's terminal, opened afresh and driven by the reactor.
    Reopened(AsyncTty),
    /// The inherited stdout, written on the blocking pool.
    ///
    /// Whatever else shares its description may have made it non-blocking, so a full buffer is
    /// waited out rather than taken as an error (#4217).
    Inherited(Arc<OwnedFd>),
}

impl Stdout {
    /// Write all of `data`, waiting for as long as the terminal takes to accept it.
    async fn write_all(&mut self, data: &[u8]) -> std::io::Result<()> {
        match self {
            Self::Reopened(tty) => tty.write_all(data).await,
            Self::Inherited(fd) => {
                let (fd, data) = (Arc::clone(fd), data.to_vec());
                tokio::task::spawn_blocking(move || fd.write_all_retrying(&data))
                    .await
                    .map_err(std::io::Error::other)?
                    .map_err(std::io::Error::from)
            }
        }
    }
}

/// Spawn a task that periodically updates the socket's modification time so it doesn't get
/// automatically deleted by temporary file cleaners.
fn spawn_socket_updater(path: PathBuf) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(SOCKET_KEEPALIVE_INTERVAL).await;
            if let Err(e) = atuin_common::os::unix::touch_file(&path)
                && e.kind() == std::io::ErrorKind::NotFound
            {
                break;
            }
        }
    })
}

fn spawn_resize_handler(
    master: Box<dyn MasterPty + Send>,
    resize_tx: mpsc::Sender<Msg>,
) -> std::io::Result<()> {
    let mut signals = signal(SignalKind::window_change())?;

    tokio::spawn(async move {
        while signals.recv().await.is_some() {
            if let Ok((cols, rows)) = terminal::size() {
                let _ = master.resize(PtySize {
                    rows,
                    cols,
                    pixel_width: 0,
                    pixel_height: 0,
                });
                let _ = resize_tx.send(Msg::Resize { rows, cols }).await;
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
    use std::ffi::OsStr;
    use std::io::{Read as _, Write as _};
    use std::path::Path;
    use std::sync::Arc;
    use std::time::Duration;

    use easy_cast::Conv;
    use rstest::rstest;

    use super::{CommandBuilder, Stdin, Stdout, process_exit_code, set_child_env};

    #[rstest]
    #[case::zero(0, 0)]
    #[case::mid_range(127, 127)]
    #[case::max_i32(u32::conv(i32::MAX), i32::MAX)]
    #[case::overflow_defaults_to_one(u32::conv(i32::MAX) + 1, 1)]
    fn maps_exit_code(#[case] input: u32, #[case] expected: i32) {
        assert_eq!(process_exit_code(input), expected);
    }

    #[rstest]
    fn a_proxy_without_a_socket_tells_its_shell_not_to_start_another() {
        // The shell cannot detect a proxy that has no socket, so it would exec a replacement
        // that fails in exactly the same way, forever.
        let mut cmd = CommandBuilder::new("true");

        set_child_env(&mut cmd, None);

        assert_eq!(cmd.get_env("ATUIN_PTY_PROXY_FAILED"), Some(OsStr::new("1")));
    }

    #[rstest]
    fn a_working_proxy_clears_an_inherited_failed_flag() {
        // An outer proxy that failed exports the flag. A working proxy nested below it -- in a
        // multiplexer pane, say -- must not pass that verdict on to its own shell.
        let mut cmd = CommandBuilder::new("true");
        cmd.env("ATUIN_PTY_PROXY_FAILED", "1");

        set_child_env(&mut cmd, Some(Path::new("/tmp/atuin-test/pty-proxy-1-2-3.sock")));

        assert_eq!(cmd.get_env("ATUIN_PTY_PROXY_FAILED"), None);
    }

    #[rstest]
    #[tokio::test]
    async fn inherited_stdout_waits_out_a_full_non_blocking_pipe() {
        // Whatever shares the inherited description can leave it non-blocking (#4217). A full
        // buffer has to hold the output back, not end it.
        let (mut reader, writer) = std::io::pipe().unwrap();
        rustix::io::ioctl_fionbio(&writer, true).unwrap();
        let mut stdout = Stdout::Inherited(Arc::new(writer.into()));
        let collector = std::thread::spawn(move || {
            // Long enough for the write to fill the pipe and find it full.
            std::thread::sleep(Duration::from_millis(200));
            let mut got = Vec::new();
            reader.read_to_end(&mut got).unwrap();
            got
        });

        let data = vec![0xAC; 512 * 1024];
        stdout.write_all(&data).await.unwrap();
        drop(stdout); // EOF for the reader

        assert_eq!(collector.join().unwrap(), data);
    }

    #[rstest]
    #[tokio::test]
    async fn inherited_stdin_waits_for_input_on_a_non_blocking_pipe() {
        // Whatever shares the inherited description can leave it non-blocking. Nothing to read
        // yet is not the end of input, which would hand the shell an EOF and end the session.
        let (reader, mut writer) = std::io::pipe().unwrap();
        rustix::io::ioctl_fionbio(&reader, true).unwrap();
        let mut stdin = Stdin::Inherited(Arc::new(reader.into()));
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            writer.write_all(b"typed").unwrap();
        });

        let mut buf = [0u8; 64];
        let n = stdin.read(&mut buf).await.unwrap();

        assert_eq!(&buf[..n], b"typed");
    }
}
