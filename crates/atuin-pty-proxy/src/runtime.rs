use std::io::Read;
use std::io::Write;
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{Arc, Mutex};

use crossterm::terminal;
use portable_pty::{CommandBuilder, PtySize, native_pty_system};

use crate::CommandCaptureSink;
use crate::capture::CommandCaptureTracker;
use crate::compositor::{Compositor, OverlayFlags, lock_unpoisoned};
use crate::debug::{Osc133DebugHighlighter, RESET};
use crate::pty_proxy::RuntimeOptions;
use crate::screen;

pub(crate) fn main(options: RuntimeOptions) {
    if let Err(e) = run(options) {
        let _ = terminal::disable_raw_mode();
        eprintln!("atuin pty-proxy: {e:#}");
        std::process::exit(1);
    }
}

fn run(mut options: RuntimeOptions) -> eyre::Result<()> {
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
    if let Some(mask) = options.hooks.child_umask {
        cmd.umask(Some(mask as _));
    }

    let mut child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|e| eyre::eyre!("{e:#}"))?;

    drop(pair.slave);

    let pty_reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| eyre::eyre!("{e:#}"))?;
    let pty_writer = pair
        .master
        .take_writer()
        .map_err(|e| eyre::eyre!("{e:#}"))?;

    let current_cols = Arc::new(AtomicU16::new(cols.max(1)));

    // All terminal writes go through the compositor, which also maintains
    // the socket-served screen model; splitting only matters when an
    // overlay may paint.
    let flags = Arc::new(OverlayFlags::default());
    let compositor = Arc::new(Mutex::new(Compositor::new(
        rows,
        cols,
        std::io::stdout(),
        flags.clone(),
        options.hooks.suggestion_provider.is_some(),
    )));

    screen::spawn_socket_server(sock_path.clone(), compositor.clone());
    spawn_resize_handler(pair.master, compositor.clone(), current_cols.clone())?;

    let (mut input_tracker, key_filter) = options
        .hooks
        .suggestion_provider
        .take()
        .map(|provider| {
            let handles =
                crate::suggest::spawn(provider, compositor.clone(), flags, current_cols.clone());
            (handles.tracker, handles.keys)
        })
        .unzip();

    terminal::enable_raw_mode()?;
    seed_cursor_from_terminal(&compositor, &mut pty_writer);

    let pump_compositor = compositor;
    let stdout_thread = std::thread::spawn(move || {
        let mut highlighter = options.debug_osc133.then(Osc133DebugHighlighter::new);
        let mut capture_tracker = options
            .hooks
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
                        options.hooks.command_capture_sink.as_ref(),
                    ) {
                        tracker.push(&buf[..n], sink);
                    }

                    let mut compositor = lock_unpoisoned(&pump_compositor);
                    let written = if let Some(highlighter) = highlighter.as_mut() {
                        let rendered = highlighter.render(&buf[..n]);
                        compositor.apply_pty(&rendered)
                    } else {
                        compositor.apply_pty(&buf[..n])
                    };
                    drop(compositor);
                    if written.is_err() {
                        break;
                    }

                    // After the chunk is applied to screen and model alike,
                    // so suggestion repaints see this chunk everywhere.
                    if let Some(tracker) = input_tracker.as_mut() {
                        tracker.push(&buf[..n]);
                    }
                }
            }
        }

        let mut compositor = lock_unpoisoned(&pump_compositor);
        compositor.flush_pending();
        if highlighter.is_some() {
            let _ = compositor.apply_pty(RESET);
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
                    let forwarded = match key_filter.as_ref() {
                        Some(filter) => {
                            let out = filter.process(&buf[..n], &mut stdin);
                            if out.is_empty() {
                                Ok(())
                            } else {
                                pty_writer.write_all(&out)
                            }
                        }
                        None => pty_writer.write_all(&buf[..n]),
                    };
                    if forwarded.is_err() {
                        break;
                    }
                }
            }
        }
    });
}

/// How long to wait for the terminal's cursor-position report. Real
/// terminals answer within a few milliseconds; on a miss the proxy runs
/// with an unseeded model, as before.
const CPR_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(300);
const CPR_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(5);

/// Ask the terminal where its cursor is (`ESC[6n`) and seed the screen
/// model with the answer, so overlays land on the prompt the user sees
/// even when the proxy starts on a non-empty screen. Runs before the
/// pumps, in raw mode; anything the user typed ahead of the reply is
/// forwarded to the pty untouched.
fn seed_cursor_from_terminal(
    compositor: &Arc<Mutex<Compositor<std::io::Stdout>>>,
    pty_writer: &mut (impl Write + ?Sized),
) {
    let mut stdout = std::io::stdout();
    if stdout
        .write_all(b"\x1b[6n")
        .and_then(|()| stdout.flush())
        .is_err()
    {
        return;
    }

    let mut stdin = std::io::stdin();
    let deadline = std::time::Instant::now() + CPR_TIMEOUT;
    let mut pending = Vec::new();
    let mut buf = [0u8; 256];
    loop {
        if rustix::io::ioctl_fionread(&stdin).unwrap_or(0) == 0 {
            if std::time::Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(CPR_POLL_INTERVAL);
            continue;
        }
        let Ok(n) = stdin.read(&mut buf) else { break };
        if n == 0 {
            break;
        }
        pending.extend_from_slice(&buf[..n]);
        if let Some((row, col)) = take_cursor_report(&mut pending) {
            lock_unpoisoned(compositor).seed_cursor(row, col);
            break;
        }
    }

    if !pending.is_empty() {
        let _ = pty_writer.write_all(&pending);
    }
}

/// Find and remove the first `ESC[row;colR` cursor-position report,
/// returning its 1-based coordinates. Keystrokes racing the report are
/// left in place.
fn take_cursor_report(bytes: &mut Vec<u8>) -> Option<(u16, u16)> {
    for start in 0..bytes.len() {
        if bytes[start] != 0x1b {
            continue;
        }
        if let Some((len, row, col)) = parse_cursor_report(&bytes[start..]) {
            bytes.drain(start..start + len);
            return Some((row, col));
        }
    }
    None
}

fn parse_cursor_report(bytes: &[u8]) -> Option<(usize, u16, u16)> {
    let rest = bytes.strip_prefix(b"\x1b[")?;
    let end = rest.iter().position(|&b| b == b'R')?;
    let (row, col) = std::str::from_utf8(&rest[..end]).ok()?.split_once(';')?;
    Some((2 + end + 1, row.parse().ok()?, col.parse().ok()?))
}

fn spawn_resize_handler(
    master: Box<dyn portable_pty::MasterPty + Send>,
    compositor: Arc<Mutex<Compositor<std::io::Stdout>>>,
    current_cols: Arc<AtomicU16>,
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
                lock_unpoisoned(&compositor).resize(rows, cols);
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
    use super::{process_exit_code, take_cursor_report};
    use rstest::rstest;

    #[rstest]
    #[case::clean(b"\x1b[24;1R".to_vec(), Some((24, 1)), b"".to_vec())]
    #[case::typed_ahead(b"ls\x1b[3;80R".to_vec(), Some((3, 80)), b"ls".to_vec())]
    #[case::arrow_key_first(
        b"\x1b[A\x1b[12;5R".to_vec(),
        Some((12, 5)),
        b"\x1b[A".to_vec()
    )]
    #[case::incomplete(b"\x1b[24;1".to_vec(), None, b"\x1b[24;1".to_vec())]
    #[case::no_report(b"plain keys".to_vec(), None, b"plain keys".to_vec())]
    fn extracts_cursor_report(
        #[case] mut bytes: Vec<u8>,
        #[case] expected: Option<(u16, u16)>,
        #[case] remainder: Vec<u8>,
    ) {
        assert_eq!(take_cursor_report(&mut bytes), expected);
        assert_eq!(bytes, remainder);
    }

    #[rstest]
    #[case::zero(0, 0)]
    #[case::mid_range(127, 127)]
    #[case::max_i32(i32::MAX as u32, i32::MAX)]
    #[case::overflow_defaults_to_one(i32::MAX as u32 + 1, 1)]
    fn maps_exit_code(#[case] input: u32, #[case] expected: i32) {
        assert_eq!(process_exit_code(input), expected);
    }
}
