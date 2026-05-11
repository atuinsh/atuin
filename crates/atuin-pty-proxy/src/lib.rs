pub mod osc133;

use clap::{Args, Subcommand, ValueEnum};

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// Print shell code to initialize atuin pty-proxy on shell startup
    Init(Init),
}

#[derive(Args, Debug)]
pub struct Init {
    /// Shell to generate init for. If omitted, attempt auto-detection
    #[arg(value_enum)]
    shell: Option<Shell>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
#[value(rename_all = "lower")]
#[allow(clippy::enum_variant_names, clippy::doc_markdown)]
enum Shell {
    /// Zsh setup
    Zsh,
    /// Bash setup
    Bash,
    /// Fish setup
    Fish,
    /// Nu setup
    Nu,
}

impl Init {
    fn run(self) -> Result<(), String> {
        let shell = detect_shell(self.shell)?;
        let script = render_init(shell);
        print!("{script}");
        Ok(())
    }
}

pub fn run(cmd: Option<Cmd>) {
    match cmd {
        Some(Cmd::Init(init)) => {
            if let Err(err) = init.run() {
                eprintln!("atuin pty-proxy: {err}");
                std::process::exit(1);
            }
        }
        None => app::main(),
    }
}

fn detect_shell(cli_shell: Option<Shell>) -> Result<Shell, String> {
    if let Some(shell) = cli_shell {
        return Ok(shell);
    }

    if let Ok(shell) = std::env::var("ATUIN_SHELL")
        && let Some(shell) = shell_from_name(&shell)
    {
        return Ok(shell);
    }

    if let Ok(shell) = std::env::var("SHELL")
        && let Some(shell) = shell_from_name(&shell)
    {
        return Ok(shell);
    }

    Err(
        "could not detect a supported shell. Please specify one explicitly: bash, zsh, fish, or nu"
            .to_string(),
    )
}

fn shell_from_name(name: &str) -> Option<Shell> {
    let shell = name
        .trim()
        .rsplit('/')
        .next()
        .unwrap_or(name)
        .trim_start_matches('-')
        .to_ascii_lowercase();

    match shell.as_str() {
        "bash" => Some(Shell::Bash),
        "zsh" => Some(Shell::Zsh),
        "fish" => Some(Shell::Fish),
        "nu" => Some(Shell::Nu),
        _ => None,
    }
}

fn render_init(shell: Shell) -> &'static str {
    match shell {
        Shell::Bash | Shell::Zsh => {
            r#"if [[ "$-" == *i* ]] && [[ -t 0 ]] && [[ -t 1 ]]; then
  _atuin_pty_proxy_tmux_current="${TMUX:-}"
  _atuin_pty_proxy_tmux_previous="${ATUIN_PTY_PROXY_TMUX:-${ATUIN_HEX_TMUX:-}}"

  if [[ -z "${ATUIN_PTY_PROXY_ACTIVE:-${ATUIN_HEX_ACTIVE:-}}" ]] || [[ "$_atuin_pty_proxy_tmux_current" != "$_atuin_pty_proxy_tmux_previous" ]]; then
    export ATUIN_PTY_PROXY_ACTIVE=1
    export ATUIN_PTY_PROXY_TMUX="$_atuin_pty_proxy_tmux_current"
    exec atuin pty-proxy
  fi

  unset _atuin_pty_proxy_tmux_current _atuin_pty_proxy_tmux_previous
fi
"#
        }
        Shell::Fish => {
            r#"if status is-interactive; and test -t 0; and test -t 1
    set -l _atuin_pty_proxy_tmux_current ""
    if set -q TMUX
        set _atuin_pty_proxy_tmux_current "$TMUX"
    end

    set -l _atuin_pty_proxy_tmux_previous ""
    if set -q ATUIN_PTY_PROXY_TMUX
        set _atuin_pty_proxy_tmux_previous "$ATUIN_PTY_PROXY_TMUX"
    else if set -q ATUIN_HEX_TMUX
        set _atuin_pty_proxy_tmux_previous "$ATUIN_HEX_TMUX"
    end

    if not set -q ATUIN_PTY_PROXY_ACTIVE; and not set -q ATUIN_HEX_ACTIVE
        set -gx ATUIN_PTY_PROXY_ACTIVE 1
        set -gx ATUIN_PTY_PROXY_TMUX "$_atuin_pty_proxy_tmux_current"
        exec atuin pty-proxy
    else if test "$_atuin_pty_proxy_tmux_current" != "$_atuin_pty_proxy_tmux_previous"
        set -gx ATUIN_PTY_PROXY_ACTIVE 1
        set -gx ATUIN_PTY_PROXY_TMUX "$_atuin_pty_proxy_tmux_current"
        exec atuin pty-proxy
    end
end
"#
        }
        // Nushell cannot dynamically source the output of `atuin init nu`,
        // so we only output the pty-proxy preamble here. Users must also set up
        // `atuin init nu` separately.
        Shell::Nu => {
            r#"if (is-terminal --stdin) and (is-terminal --stdout) {
    let tmux_current = ($env.TMUX? | default "")
    let tmux_previous = ($env.ATUIN_PTY_PROXY_TMUX? | default ($env.ATUIN_HEX_TMUX? | default ""))

    if (($env.ATUIN_PTY_PROXY_ACTIVE? | default ($env.ATUIN_HEX_ACTIVE? | default "")) | is-empty) or ($tmux_current != $tmux_previous) {
        $env.ATUIN_PTY_PROXY_ACTIVE = "1"
        $env.ATUIN_PTY_PROXY_TMUX = $tmux_current
        exec atuin pty-proxy
    }
}
"#
        }
    }
}

#[cfg(not(unix))]
mod app {
    pub(crate) fn main() {
        eprintln!("atuin pty-proxy currently supports unix platforms");
        std::process::exit(1);
    }
}

#[cfg(unix)]
mod app {
    use std::io::{Read, Write};
    use std::os::unix::net::UnixListener;
    use std::sync::mpsc;

    use crossterm::terminal;
    use portable_pty::{CommandBuilder, PtySize, native_pty_system};

    enum ParserMsg {
        Data(Vec<u8>),
        Resize { rows: u16, cols: u16 },
        ScreenRequest(mpsc::Sender<Vec<u8>>),
    }

    pub(crate) fn main() {
        if let Err(e) = run() {
            let _ = terminal::disable_raw_mode();
            eprintln!("atuin pty-proxy: {e:#}");
            std::process::exit(1);
        }
    }

    fn socket_path() -> std::path::PathBuf {
        let dir = std::env::temp_dir();
        dir.join(format!("atuin-pty-proxy-{}.sock", std::process::id()))
    }

    /// Wire format written to the Unix socket:
    ///
    /// ```text
    /// [rows: u16 BE][cols: u16 BE][cursor_row: u16 BE][cursor_col: u16 BE]
    /// [row_0_len: u32 BE][row_0_bytes...]
    /// [row_1_len: u32 BE][row_1_bytes...]
    /// ...
    /// ```
    ///
    /// Each row's bytes come from `screen.rows_formatted(0, cols)` and contain
    /// pre-built ANSI escape sequences.  The client can write them directly to
    /// stdout without needing its own vt100 parser.
    fn encode_screen(parser: &vt100::Parser) -> Vec<u8> {
        let screen = parser.screen();
        let (rows, cols) = screen.size();
        let (cursor_row, cursor_col) = screen.cursor_position();

        let mut buf: Vec<u8> = Vec::with_capacity(256 + (rows as usize * cols as usize));
        buf.extend_from_slice(&rows.to_be_bytes());
        buf.extend_from_slice(&cols.to_be_bytes());
        buf.extend_from_slice(&cursor_row.to_be_bytes());
        buf.extend_from_slice(&cursor_col.to_be_bytes());

        for row_bytes in screen.rows_formatted(0, cols) {
            let len = row_bytes.len() as u32;
            buf.extend_from_slice(&len.to_be_bytes());
            buf.extend_from_slice(&row_bytes);
        }

        buf
    }

    fn handle_parser_msg(parser: &mut vt100::Parser, msg: ParserMsg) {
        match msg {
            ParserMsg::Data(data) => parser.process(&data),
            ParserMsg::Resize { rows, cols } => parser.screen_mut().set_size(rows, cols),
            ParserMsg::ScreenRequest(reply_tx) => {
                let _ = reply_tx.send(encode_screen(parser));
            }
        }
    }

    fn run() -> eyre::Result<()> {
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

        // Set up socket path and expose it to child processes
        let sock_path = socket_path();
        // Clean up any stale socket from a previous crash
        let _ = std::fs::remove_file(&sock_path);

        let mut cmd = CommandBuilder::new_default_prog();
        cmd.cwd(std::env::current_dir()?);
        cmd.env("ATUIN_PTY_PROXY_SOCKET", sock_path.as_os_str());
        cmd.env("ATUIN_HEX_SOCKET", sock_path.as_os_str());

        let mut child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| eyre::eyre!("{e:#}"))?;

        // Close slave side in parent process
        drop(pair.slave);

        let mut pty_reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| eyre::eyre!("{e:#}"))?;
        let mut pty_writer = pair
            .master
            .take_writer()
            .map_err(|e| eyre::eyre!("{e:#}"))?;

        // Channel: stdout/sigwinch/socket threads -> parser thread (bounded, non-blocking send)
        let (msg_tx, msg_rx) = mpsc::sync_channel::<ParserMsg>(64);

        // --- Parser thread ---
        // Maintains a persistent vt100::Parser fed bytes as they arrive.
        // On screen request: reads current state directly (no replay).
        std::thread::spawn(move || {
            let mut parser = vt100::Parser::new(rows, cols, 0);

            loop {
                // Block until at least one message arrives
                let first = match msg_rx.recv() {
                    Ok(msg) => msg,
                    Err(_) => break,
                };

                handle_parser_msg(&mut parser, first);

                // Drain all remaining pending messages so the parser stays
                // caught up during high-throughput bursts (e.g. `cat bigfile`).
                // The channel holds at most 64 items, so this is bounded.
                while let Ok(msg) = msg_rx.try_recv() {
                    handle_parser_msg(&mut parser, msg);
                }
            }
        });

        // --- Socket server thread ---
        // Listens on Unix socket; on connection, requests screen state from parser thread.
        {
            let sock_path_clone = sock_path.clone();
            let screen_tx = msg_tx.clone();
            std::thread::spawn(move || {
                let listener = match UnixListener::bind(&sock_path_clone) {
                    Ok(l) => l,
                    Err(e) => {
                        eprintln!("atuin pty-proxy: failed to bind socket: {e}");
                        return;
                    }
                };

                for stream in listener.incoming() {
                    let mut stream = match stream {
                        Ok(s) => s,
                        Err(_) => break,
                    };

                    let (reply_tx, reply_rx) = mpsc::channel();
                    if screen_tx.send(ParserMsg::ScreenRequest(reply_tx)).is_err() {
                        break;
                    }
                    if let Ok(data) = reply_rx.recv() {
                        let _ = stream.write_all(&data);
                        let _ = stream.flush();
                    }
                }
            });
        }

        // Handle terminal resize via SIGWINCH
        {
            use signal_hook::consts::SIGWINCH;
            use signal_hook::iterator::Signals;

            let master = pair.master;
            let resize_tx = msg_tx.clone();
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
                        let _ = resize_tx.try_send(ParserMsg::Resize { rows, cols });
                    }
                }
            });
        }

        terminal::enable_raw_mode()?;

        // PTY -> stdout (with OSC 133 parsing + buffer feed)
        let stdout_thread = std::thread::spawn(move || {
            let mut stdout = std::io::stdout();
            let mut parser = crate::osc133::Parser::new();
            let mut buf = [0u8; 8192];
            loop {
                match pty_reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        parser.push(&buf[..n], |_event| {
                            // Zone transitions are tracked inside the parser.
                            // Callers can query parser.zone() after push.
                        });

                        // Feed bytes to the shadow parser. Drops on backpressure —
                        // the screen snapshot may be stale during bursts, but
                        // self-corrects once output settles.
                        let _ = msg_tx.try_send(ParserMsg::Data(buf[..n].to_vec()));

                        if stdout.write_all(&buf[..n]).is_err() {
                            break;
                        }
                        let _ = stdout.flush();
                    }
                }
            }
        });

        // stdin -> PTY
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

        // Clean up socket file
        let _ = std::fs::remove_file(&sock_path);

        std::process::exit(process_exit_code(status.exit_code()));
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
}

#[cfg(test)]
mod tests {
    use super::{Shell, render_init, shell_from_name};

    #[test]
    fn shell_from_name_handles_paths() {
        assert_eq!(shell_from_name("/bin/zsh"), Some(Shell::Zsh));
        assert_eq!(shell_from_name("/usr/local/bin/bash"), Some(Shell::Bash));
        assert_eq!(shell_from_name("fish"), Some(Shell::Fish));
        assert_eq!(shell_from_name("nu"), Some(Shell::Nu));
    }

    #[test]
    fn posix_init_uses_exec_and_tmux_guard() {
        let script = render_init(Shell::Bash);
        assert!(script.contains("exec atuin pty-proxy"));
        assert!(script.contains("ATUIN_PTY_PROXY_TMUX"));
        assert!(!script.contains("eval \"$(atuin init bash)\""));
    }

    #[test]
    fn posix_init_has_no_double_braces() {
        let script = render_init(Shell::Bash);
        assert!(!script.contains("${{"), "double braces in bash init script");
    }

    #[test]
    fn fish_init_uses_source() {
        let script = render_init(Shell::Fish);
        assert!(script.contains("exec atuin pty-proxy"));
        assert!(!script.contains("atuin init fish | source"));
    }

    #[test]
    fn nu_init_uses_exec_and_tty_guard() {
        let script = render_init(Shell::Nu);
        assert!(script.contains("exec atuin pty-proxy"));
        assert!(script.contains("ATUIN_PTY_PROXY_TMUX"));
        assert!(script.contains("is-terminal --stdin"));
        assert!(script.contains("is-terminal --stdout"));
        assert!(script.contains("ATUIN_PTY_PROXY_ACTIVE"));
    }
}
