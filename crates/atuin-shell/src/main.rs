mod osc133;

use clap::{Args, Parser, Subcommand, ValueEnum};

#[derive(Parser, Debug)]
#[command(infer_subcommands = true)]
struct Cli {
    #[command(subcommand)]
    command: Option<Cmd>,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Print shell code to initialize atuin-shell on shell startup
    Init(Init),
}

#[derive(Args, Debug)]
struct Init {
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
}

impl Shell {
    fn as_str(self) -> &'static str {
        match self {
            Self::Bash => "bash",
            Self::Zsh => "zsh",
            Self::Fish => "fish",
        }
    }
}

impl Init {
    fn run(self) -> Result<(), String> {
        let shell = detect_shell(self.shell)?;
        let script = render_init(shell);
        print!("{script}");
        Ok(())
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
        "could not detect a supported shell. Please specify one explicitly: bash, zsh, or fish"
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
        _ => None,
    }
}

fn init_command(shell: Shell) -> String {
    format!("atuin init {}", shell.as_str())
}

fn render_init(shell: Shell) -> String {
    let init_command = init_command(shell);

    match shell {
        Shell::Bash | Shell::Zsh => format!(
            r#"if [[ "$-" == *i* ]] && [[ -t 0 ]] && [[ -t 1 ]]; then
  _atuin_shell_tmux_current="${{TMUX:-}}"
  _atuin_shell_tmux_previous="${{ATUIN_SHELL_TMUX:-}}"

  if [[ -z "${{ATUIN_SHELL_ACTIVE:-}}" ]] || [[ "$_atuin_shell_tmux_current" != "$_atuin_shell_tmux_previous" ]]; then
    export ATUIN_SHELL_ACTIVE=1
    export ATUIN_SHELL_TMUX="$_atuin_shell_tmux_current"
    exec atuin-shell
  fi

  unset _atuin_shell_tmux_current _atuin_shell_tmux_previous
fi

eval "$({init_command})"
"#
        ),
        Shell::Fish => format!(
            r#"if status is-interactive; and test -t 0; and test -t 1
    set -l _atuin_shell_tmux_current ""
    if set -q TMUX
        set _atuin_shell_tmux_current "$TMUX"
    end

    set -l _atuin_shell_tmux_previous ""
    if set -q ATUIN_SHELL_TMUX
        set _atuin_shell_tmux_previous "$ATUIN_SHELL_TMUX"
    end

    if not set -q ATUIN_SHELL_ACTIVE
        set -gx ATUIN_SHELL_ACTIVE 1
        set -gx ATUIN_SHELL_TMUX "$_atuin_shell_tmux_current"
        exec atuin-shell
    else if test "$_atuin_shell_tmux_current" != "$_atuin_shell_tmux_previous"
        set -gx ATUIN_SHELL_ACTIVE 1
        set -gx ATUIN_SHELL_TMUX "$_atuin_shell_tmux_current"
        exec atuin-shell
    end
end

{init_command} | source
"#
        ),
    }
}

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Some(Cmd::Init(init)) => {
            if let Err(err) = init.run() {
                eprintln!("atuin-shell: {err}");
                std::process::exit(1);
            }
        }
        None => app::main(),
    }
}

#[cfg(any(not(unix), target_os = "illumos"))]
mod app {
    pub(crate) fn main() {
        eprintln!("atuin-shell currently supports unix platforms excluding illumos");
        std::process::exit(1);
    }
}

#[cfg(all(unix, not(target_os = "illumos")))]
mod app {
    use std::collections::VecDeque;
    use std::io::{Read, Write};
    use std::os::unix::net::UnixListener;
    use std::sync::mpsc;

    use crossterm::terminal;
    use portable_pty::{CommandBuilder, PtySize, native_pty_system};

    /// Default ring buffer capacity (2 MB).
    const DEFAULT_RING_BUFFER_CAP: usize = 2 * 1024 * 1024;

    pub(crate) fn main() {
        if let Err(e) = run() {
            let _ = terminal::disable_raw_mode();
            eprintln!("atuin-shell: {e:#}");
            std::process::exit(1);
        }
    }

    fn ring_buffer_cap() -> usize {
        std::env::var("ATUIN_SCREEN_BUFFER_SIZE")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(DEFAULT_RING_BUFFER_CAP)
    }

    fn socket_path() -> std::path::PathBuf {
        let dir = std::env::temp_dir();
        dir.join(format!("atuin-shell-{}.sock", std::process::id()))
    }

    /// Wire format written to the Unix socket:
    /// [rows: u16 BE][cols: u16 BE][cursor_row: u16 BE][cursor_col: u16 BE][cell_bytes...]
    ///
    /// `cell_bytes` is the screen contents rendered as ANSI-styled text (one row
    /// per line, separated by `\n`).  The client can replay it through its own
    /// `vt100::Parser` to recover per-cell attributes.
    fn encode_screen(parser: &vt100::Parser) -> Vec<u8> {
        let screen = parser.screen();
        let (rows, cols) = screen.size();
        let (cursor_row, cursor_col) = screen.cursor_position();

        let mut buf: Vec<u8> = Vec::with_capacity(256 + (rows as usize * cols as usize));
        buf.extend_from_slice(&rows.to_be_bytes());
        buf.extend_from_slice(&cols.to_be_bytes());
        buf.extend_from_slice(&cursor_row.to_be_bytes());
        buf.extend_from_slice(&cursor_col.to_be_bytes());

        // contents_formatted gives ANSI-escaped bytes representing the full screen
        buf.extend_from_slice(&screen.contents_formatted());

        buf
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
        cmd.env("ATUIN_SHELL_SOCKET", sock_path.as_os_str());

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

        // Channel: stdout thread -> buffer thread (bounded, non-blocking send)
        let (byte_tx, byte_rx) = mpsc::sync_channel::<Vec<u8>>(64);

        // Channel: screen request (oneshot pattern)
        // The requester sends a Sender<Vec<u8>> that the buffer thread uses to reply.
        let (screen_req_tx, screen_req_rx) = mpsc::channel::<mpsc::Sender<Vec<u8>>>();

        // --- Buffer thread ---
        // Maintains a ring buffer of raw PTY output bytes.
        // On screen request: replays through vt100::Parser to produce screen state.
        std::thread::spawn(move || {
            let cap = ring_buffer_cap();
            let mut ring: VecDeque<u8> = VecDeque::with_capacity(cap);

            loop {
                // Check for screen requests first (non-blocking)
                match screen_req_rx.try_recv() {
                    Ok(reply_tx) => {
                        // Replay ring buffer through a fresh vt100 parser
                        let (cols, rows) =
                            terminal::size().unwrap_or((80, 24));
                        let mut parser = vt100::Parser::new(rows, cols, 0);

                        // Feed the ring buffer contents in order
                        let (front, back) = ring.as_slices();
                        parser.process(front);
                        parser.process(back);

                        let encoded = encode_screen(&parser);
                        let _ = reply_tx.send(encoded);
                    }
                    Err(mpsc::TryRecvError::Empty) => {}
                    Err(mpsc::TryRecvError::Disconnected) => break,
                }

                // Wait for bytes from stdout thread (with timeout so we can
                // also service screen requests promptly)
                match byte_rx.recv_timeout(std::time::Duration::from_millis(50)) {
                    Ok(data) => {
                        // Append to ring buffer, evicting oldest bytes if over capacity
                        if ring.len() + data.len() > cap {
                            let to_drain = (ring.len() + data.len()).saturating_sub(cap);
                            ring.drain(..to_drain);
                        }
                        ring.extend(&data);
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
        });

        // --- Socket server thread ---
        // Listens on Unix socket; on connection, requests screen state from buffer thread.
        {
            let sock_path_clone = sock_path.clone();
            let screen_req_tx = screen_req_tx;
            std::thread::spawn(move || {
                let listener = match UnixListener::bind(&sock_path_clone) {
                    Ok(l) => l,
                    Err(e) => {
                        eprintln!("atuin-shell: failed to bind socket: {e}");
                        return;
                    }
                };

                // Non-blocking so we can detect when the process is exiting
                // (channel disconnect) without blocking forever.
                let _ = listener.set_nonblocking(true);

                loop {
                    match listener.accept() {
                        Ok((mut stream, _)) => {
                            let _ = stream.set_nonblocking(false);
                            // Request screen state from buffer thread
                            let (reply_tx, reply_rx) = mpsc::channel();
                            if screen_req_tx.send(reply_tx).is_err() {
                                break; // buffer thread gone
                            }
                            if let Ok(data) = reply_rx.recv() {
                                let _ = stream.write_all(&data);
                                let _ = stream.flush();
                            }
                        }
                        Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            // Check if request channel is still alive
                            // (We can't send without a real request, so just sleep briefly)
                            std::thread::sleep(std::time::Duration::from_millis(50));
                            continue;
                        }
                        Err(_) => break,
                    }
                }
            });
        }

        // Handle terminal resize via SIGWINCH
        {
            use signal_hook::consts::SIGWINCH;
            use signal_hook::iterator::Signals;

            let master = pair.master;
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

                        // Feed bytes to ring buffer (non-blocking, drops if full)
                        let _ = byte_tx.try_send(buf[..n].to_vec());

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
    use super::{Shell, init_command, render_init, shell_from_name};

    #[test]
    fn shell_from_name_handles_paths() {
        assert_eq!(shell_from_name("/bin/zsh"), Some(Shell::Zsh));
        assert_eq!(shell_from_name("/usr/local/bin/bash"), Some(Shell::Bash));
        assert_eq!(shell_from_name("fish"), Some(Shell::Fish));
    }

    #[test]
    fn init_command_is_bootstrap_only() {
        let command = init_command(Shell::Zsh);
        assert_eq!(command, "atuin init zsh");
    }

    #[test]
    fn posix_init_uses_exec_and_tmux_guard() {
        let script = render_init(Shell::Bash);
        assert!(script.contains("exec atuin-shell"));
        assert!(script.contains("ATUIN_SHELL_TMUX"));
        assert!(script.contains("eval \"$(atuin init bash)\""));
    }

    #[test]
    fn fish_init_uses_source() {
        let script = render_init(Shell::Fish);
        assert!(script.contains("exec atuin-shell"));
        assert!(script.contains("atuin init fish | source"));
    }
}
