use std::io::{Read, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::mpsc;

use crossterm::terminal;
use portable_pty::{CommandBuilder, PtySize, native_pty_system};

use crate::capture::CommandCaptureTracker;
use crate::debug::{Osc133DebugHighlighter, RESET};
use crate::pty_proxy::RuntimeOptions;
use crate::screen::{self, Msg};

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

    let mut cmd = CommandBuilder::new_default_prog();
    cmd.cwd(std::env::current_dir()?);
    cmd.env("ATUIN_PTY_PROXY_SOCKET", sock_path.as_os_str());
    cmd.env("ATUIN_PTY_PROXY_ACTIVE", "1");

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

fn spawn_resize_handler(
    master: Box<dyn portable_pty::MasterPty + Send>,
    resize_tx: mpsc::SyncSender<Msg>,
    current_cols: Arc<AtomicU16>,
) -> eyre::Result<()> {
    use signal_hook::consts::SIGWINCH;
    use signal_hook::iterator::Signals;

    let mut signals = Signals::new([SIGWINCH])?;

    std::thread::spawn(move || {
        for _ in signals.forever() {
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
    });

    Ok(())
}

fn process_exit_code(code: u32) -> i32 {
    i32::try_from(code).unwrap_or(1)
}

#[cfg(test)]
mod tests {
    use super::process_exit_code;

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
