use std::io::{Read, Write};
use std::sync::mpsc;

use crossterm::terminal;
use portable_pty::{CommandBuilder, PtySize, native_pty_system};

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

    let sock_path = match screen::socket_path() {
        Ok(path) => {
            let _ = std::fs::remove_file(&path);
            Some(path)
        }
        Err(e) => {
            // If creating the socket fails, print the error and continue rather than returning it,
            // so the user still gets a shell. This is the same behavior as when binding the socket
            // fails in `screen::spawn_socket_server`.
            eprintln!("atuin pty-proxy: failed to create socket: {e}");
            None
        }
    };

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
    if let Some(path) = &sock_path {
        cmd.env("ATUIN_PTY_PROXY_SOCKET", path);
    } else {
        cmd.env_remove("ATUIN_PTY_PROXY_SOCKET");
    }
    cmd.env("ATUIN_PTY_PROXY_ACTIVE", "1");
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

    let mut child = pair.slave.spawn_command(cmd).map_err(|e| eyre::eyre!("{e:#}"))?;
    drop(pair.slave);

    let mut pty_reader = pair.master.try_clone_reader().map_err(|e| eyre::eyre!("{e:#}"))?;
    let mut pty_writer = pair.master.take_writer().map_err(|e| eyre::eyre!("{e:#}"))?;

    let (msg_tx, msg_rx) = mpsc::sync_channel::<Msg>(64);
    let _parser_handle = screen::spawn_parser_thread(rows, cols, msg_rx, screen::ParserOptions {
        sink: options.command_capture_sink,
        debug_osc133: options.debug_osc133,
    });
    if let Some(path) = &sock_path {
        screen::spawn_socket_server(path.clone(), msg_tx.clone());
    }
    spawn_resize_handler(pair.master, msg_tx.clone())?;
    terminal::enable_raw_mode()?;

    let stdout_thread = std::thread::spawn(move || {
        let mut stdout = std::io::stdout();
        let mut highlighter = options.debug_osc133.then(Osc133DebugHighlighter::new);
        let mut buf = [0u8; 8192];

        loop {
            match pty_reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let raw_data = &buf[..n];
                    let _ = msg_tx.send(Msg::Data(raw_data.to_vec()));

                    let highlighted;
                    let data: &[u8] = if let Some(highlighter) = &mut highlighter {
                        highlighted = highlighter.render(raw_data);
                        &highlighted
                    } else {
                        raw_data
                    };

                    if stdout.write_all(data).is_err() {
                        break;
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
    if let Some(path) = &sock_path {
        let _ = std::fs::remove_file(path);
    }

    std::process::exit(process_exit_code(status.exit_code()));
}

fn spawn_resize_handler(
    master: Box<dyn portable_pty::MasterPty + Send>,
    resize_tx: mpsc::SyncSender<Msg>,
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
                let _ = resize_tx.send(Msg::Resize { rows, cols });
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
    use easy_cast::Conv;
    use rstest::rstest;

    use super::process_exit_code;

    #[rstest]
    #[case::zero(0, 0)]
    #[case::mid_range(127, 127)]
    #[case::max_i32(u32::conv(i32::MAX), i32::MAX)]
    #[case::overflow_defaults_to_one(u32::conv(i32::MAX) + 1, 1)]
    fn maps_exit_code(#[case] input: u32, #[case] expected: i32) {
        assert_eq!(process_exit_code(input), expected);
    }
}
