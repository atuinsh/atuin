use std::io::{Read, Write};
use std::path::Path;
use std::sync::mpsc;

use crossterm::terminal;
use portable_pty::{CommandBuilder, PtySize, native_pty_system};

use crate::debug::{Osc133DebugHighlighter, RESET};
use crate::pty_proxy::RuntimeOptions;
use crate::screen::{self, Msg};

/// Environment for the spawned shell: the socket path for screen requests,
/// an active flag so nested shells don't start another proxy, and the PTY
/// slave device so shells and clients can tell whether the proxy still owns
/// their terminal surface. When the slave device is unknown, any inherited
/// `ATUIN_PTY_PROXY_TTY` is removed so children never see a stale value.
fn apply_proxy_env(cmd: &mut CommandBuilder, sock_path: Option<&Path>, tty_name: Option<&Path>) {
    match sock_path {
        Some(path) => cmd.env("ATUIN_PTY_PROXY_SOCKET", path.as_os_str()),
        None => cmd.env_remove("ATUIN_PTY_PROXY_SOCKET"),
    }
    cmd.env("ATUIN_PTY_PROXY_ACTIVE", "1");
    match tty_name {
        Some(tty) => cmd.env("ATUIN_PTY_PROXY_TTY", tty.as_os_str()),
        None => cmd.env_remove("ATUIN_PTY_PROXY_TTY"),
    }
}

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
    apply_proxy_env(&mut cmd, sock_path.as_deref(), pair.master.tty_name().as_deref());
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

/// Read the current terminal size and propagate it to the child pty, the
/// column tracker, and the screen parser.
fn apply_terminal_size(master: &dyn portable_pty::MasterPty, resize_tx: &mpsc::SyncSender<Msg>) {
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

fn spawn_resize_handler(
    master: Box<dyn portable_pty::MasterPty + Send>,
    resize_tx: mpsc::SyncSender<Msg>,
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
        apply_terminal_size(&*master, &resize_tx);

        for _ in signals.forever() {
            apply_terminal_size(&*master, &resize_tx);
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

    use easy_cast::Conv;
    use portable_pty::CommandBuilder;
    use rstest::rstest;

    use super::{apply_proxy_env, process_exit_code};

    #[test]
    fn proxy_env_exports_socket_active_flag_and_tty() {
        let mut cmd = CommandBuilder::new("sh");
        apply_proxy_env(
            &mut cmd,
            Some(Path::new("/tmp/test.sock")),
            Some(Path::new("/dev/ttys009")),
        );

        assert_eq!(cmd.get_env("ATUIN_PTY_PROXY_SOCKET"), Some(OsStr::new("/tmp/test.sock")));
        assert_eq!(cmd.get_env("ATUIN_PTY_PROXY_ACTIVE"), Some(OsStr::new("1")));
        assert_eq!(cmd.get_env("ATUIN_PTY_PROXY_TTY"), Some(OsStr::new("/dev/ttys009")));
    }

    #[test]
    fn proxy_env_overrides_inherited_tty() {
        let mut cmd = CommandBuilder::new("sh");
        // Simulate a value inherited from an outer proxy's environment.
        cmd.env("ATUIN_PTY_PROXY_TTY", "/dev/ttys001");
        apply_proxy_env(
            &mut cmd,
            Some(Path::new("/tmp/test.sock")),
            Some(Path::new("/dev/ttys009")),
        );

        assert_eq!(cmd.get_env("ATUIN_PTY_PROXY_TTY"), Some(OsStr::new("/dev/ttys009")));
    }

    #[test]
    fn proxy_env_removes_inherited_tty_when_unknown() {
        let mut cmd = CommandBuilder::new("sh");
        // Simulate a value inherited from an outer proxy's environment. If
        // this proxy cannot name its own slave device, the stale path must
        // not leak through — a wrong value would make shells and popup
        // clients misjudge which terminal surface they are on.
        cmd.env("ATUIN_PTY_PROXY_TTY", "/dev/ttys001");
        apply_proxy_env(&mut cmd, Some(Path::new("/tmp/test.sock")), None);

        assert_eq!(cmd.get_env("ATUIN_PTY_PROXY_TTY"), None);
    }

    #[rstest]
    #[case::zero(0, 0)]
    #[case::mid_range(127, 127)]
    #[case::max_i32(u32::conv(i32::MAX), i32::MAX)]
    #[case::overflow_defaults_to_one(u32::conv(i32::MAX) + 1, 1)]
    fn maps_exit_code(#[case] input: u32, #[case] expected: i32) {
        assert_eq!(process_exit_code(input), expected);
    }
}
