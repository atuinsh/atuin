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
    use std::io::{Read, Write};

    use crossterm::terminal;
    use portable_pty::{CommandBuilder, PtySize, native_pty_system};

    pub(crate) fn main() {
        if let Err(e) = run() {
            let _ = terminal::disable_raw_mode();
            eprintln!("atuin-shell: {e:#}");
            std::process::exit(1);
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

        let mut cmd = CommandBuilder::new_default_prog();
        cmd.cwd(std::env::current_dir()?);
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

        // PTY -> stdout (with OSC 133 parsing)
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
