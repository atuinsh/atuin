use std::io::{Read, Write};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::mpsc;

use crossterm::terminal;
use portable_pty::{CommandBuilder, PtySize, native_pty_system};

use crate::capture::CommandCaptureTracker;
use crate::debug::{Osc133DebugHighlighter, RESET};
use crate::pty_proxy::RuntimeOptions;
use crate::screen::{self, Msg};

/// Environment for the spawned shell: the socket path for screen requests,
/// an active flag so nested shells don't start another proxy, and the PTY
/// slave device so shells and clients can tell whether the proxy still owns
/// their terminal surface. When the slave device is unknown, any inherited
/// `ATUIN_PTY_PROXY_TTY` is removed so children never see a stale value.
fn apply_proxy_env(cmd: &mut CommandBuilder, sock_path: &Path, tty_name: Option<&Path>) {
    cmd.env("ATUIN_PTY_PROXY_SOCKET", sock_path.as_os_str());
    cmd.env("ATUIN_PTY_PROXY_ACTIVE", "1");
    match tty_name {
        Some(tty) => cmd.env("ATUIN_PTY_PROXY_TTY", tty.as_os_str()),
        None => cmd.env_remove("ATUIN_PTY_PROXY_TTY"),
    }
}

pub(crate) fn main(options: RuntimeOptions) {
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

    let sock_path = screen::socket_path();
    let _ = std::fs::remove_file(&sock_path);

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
    apply_proxy_env(&mut cmd, &sock_path, pair.master.tty_name().as_deref());

    let mut child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|e| eyre::eyre!("{e:#}"))?;

    drop(pair.slave);

    let mut pty_reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| eyre::eyre!("{e:#}"))?;
    let mut pty_writer = pair
        .master
        .take_writer()
        .map_err(|e| eyre::eyre!("{e:#}"))?;

    let (msg_tx, msg_rx) = mpsc::sync_channel::<Msg>(64);
    let current_cols = Arc::new(AtomicU16::new(cols.max(1)));

    screen::spawn_parser_thread(rows, cols, msg_rx);
    screen::spawn_socket_server(sock_path.clone(), msg_tx.clone());
    spawn_resize_handler(pair.master, msg_tx.clone(), current_cols.clone())?;

    terminal::enable_raw_mode()?;

    let stdout_thread = std::thread::spawn(move || {
        let mut stdout = std::io::stdout();
        let mut highlighter = options.debug_osc133.then(Osc133DebugHighlighter::new);
        let mut capture_tracker = options
            .command_capture_sink
            .as_ref()
            .map(|_| CommandCaptureTracker::new(current_cols));
        let mut buf = [0u8; 8192];

        loop {
            match pty_reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if let (Some(tracker), Some(sink)) = (
                        capture_tracker.as_mut(),
                        options.command_capture_sink.as_ref(),
                    ) {
                        tracker.push(&buf[..n], sink);
                    }

                    if let Some(highlighter) = highlighter.as_mut() {
                        let rendered = highlighter.render(&buf[..n]);
                        let _ = msg_tx.try_send(Msg::Data(rendered.clone()));

                        if stdout.write_all(&rendered).is_err() {
                            break;
                        }
                    } else {
                        let _ = msg_tx.try_send(Msg::Data(buf[..n].to_vec()));

                        if stdout.write_all(&buf[..n]).is_err() {
                            break;
                        }
                    }
                    let _ = stdout.flush();
                }
            }
        }

        if highlighter.is_some() {
            let _ = stdout.write_all(RESET);
            let _ = stdout.flush();
        }
    });

    std::thread::spawn(move || {
        let mut stdin = std::io::stdin();
        let mut buf = [0u8; 8192];
        loop {
            match stdin.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if pty_writer.write_all(&buf[..n]).is_err() {
                        break;
                    }
                }
            }
        }
    });

    let status = child.wait()?;
    let _ = stdout_thread.join();

    let _ = terminal::disable_raw_mode();
    let _ = std::fs::remove_file(&sock_path);

    std::process::exit(process_exit_code(status.exit_code()));
}

/// Read the current terminal size and propagate it to the child pty, the
/// column tracker, and the screen parser.
fn apply_terminal_size(
    master: &dyn portable_pty::MasterPty,
    resize_tx: &mpsc::SyncSender<Msg>,
    current_cols: &AtomicU16,
) {
    if let Ok((cols, rows)) = terminal::size() {
        current_cols.store(cols.max(1), Ordering::Relaxed);
        let _ = master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        });
        let _ = resize_tx.try_send(Msg::Resize { rows, cols });
    }
}

fn spawn_resize_handler(
    master: Box<dyn portable_pty::MasterPty + Send>,
    resize_tx: mpsc::SyncSender<Msg>,
    current_cols: Arc<AtomicU16>,
) -> eyre::Result<()> {
    use signal_hook::consts::SIGWINCH;
    use signal_hook::iterator::Signals;

    // Register for SIGWINCH before spawning the thread, so any resize that
    // arrives once this returns is queued rather than lost.
    let mut signals = Signals::new([SIGWINCH])?;

    std::thread::spawn(move || {
        // The terminal may have been resized between the initial size query in
        // `run` and this handler being armed — a multiplexer settling its pane
        // layout right after spawning the shell does exactly this. That resize
        // predates the SIGWINCH registration above, so no signal is waiting for
        // it; apply the current size once up front so we don't stay stuck at a
        // stale startup size until the next resize.
        apply_terminal_size(&*master, &resize_tx, &current_cols);

        for _ in signals.forever() {
            apply_terminal_size(&*master, &resize_tx, &current_cols);
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
    use std::path::Path;

    use portable_pty::CommandBuilder;

    use super::{apply_proxy_env, process_exit_code};

    #[test]
    fn proxy_env_exports_socket_active_flag_and_tty() {
        let mut cmd = CommandBuilder::new("sh");
        apply_proxy_env(
            &mut cmd,
            Path::new("/tmp/test.sock"),
            Some(Path::new("/dev/ttys009")),
        );

        assert_eq!(
            cmd.get_env("ATUIN_PTY_PROXY_SOCKET"),
            Some(OsStr::new("/tmp/test.sock"))
        );
        assert_eq!(cmd.get_env("ATUIN_PTY_PROXY_ACTIVE"), Some(OsStr::new("1")));
        assert_eq!(
            cmd.get_env("ATUIN_PTY_PROXY_TTY"),
            Some(OsStr::new("/dev/ttys009"))
        );
    }

    #[test]
    fn proxy_env_overrides_inherited_tty() {
        let mut cmd = CommandBuilder::new("sh");
        // Simulate a value inherited from an outer proxy's environment.
        cmd.env("ATUIN_PTY_PROXY_TTY", "/dev/ttys001");
        apply_proxy_env(
            &mut cmd,
            Path::new("/tmp/test.sock"),
            Some(Path::new("/dev/ttys009")),
        );

        assert_eq!(
            cmd.get_env("ATUIN_PTY_PROXY_TTY"),
            Some(OsStr::new("/dev/ttys009"))
        );
    }

    #[test]
    fn proxy_env_removes_inherited_tty_when_unknown() {
        let mut cmd = CommandBuilder::new("sh");
        // Simulate a value inherited from an outer proxy's environment. If
        // this proxy cannot name its own slave device, the stale path must
        // not leak through — a wrong value would make shells and popup
        // clients misjudge which terminal surface they are on.
        cmd.env("ATUIN_PTY_PROXY_TTY", "/dev/ttys001");
        apply_proxy_env(&mut cmd, Path::new("/tmp/test.sock"), None);

        assert_eq!(cmd.get_env("ATUIN_PTY_PROXY_TTY"), None);
    }

    #[test]
    fn process_exit_code_preserves_valid_values() {
        assert_eq!(process_exit_code(0), 0);
        assert_eq!(process_exit_code(127), 127);
        assert_eq!(process_exit_code(i32::MAX as u32), i32::MAX);
    }

    #[test]
    fn process_exit_code_defaults_when_out_of_range() {
        assert_eq!(process_exit_code(i32::MAX as u32 + 1), 1);
    }
}
