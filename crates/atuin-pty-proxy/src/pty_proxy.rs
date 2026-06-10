use clap::{Args, Subcommand, ValueEnum};

use crate::{CommandCaptureSink, runtime};

#[derive(Args, Debug)]
pub struct PtyProxy {
    /// Highlight OSC 133 prompt, input, output, and exit-code regions
    #[arg(long)]
    debug_osc133: bool,

    #[command(subcommand)]
    cmd: Option<Cmd>,
}

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

pub(crate) struct RuntimeOptions {
    pub(crate) debug_osc133: bool,
    pub(crate) command_capture_sink: Option<CommandCaptureSink>,
}

impl RuntimeOptions {
    fn new(debug_osc133: bool, command_capture_sink: Option<CommandCaptureSink>) -> Self {
        Self {
            debug_osc133: debug_osc133 || env_flag("ATUIN_PTY_PROXY_DEBUG"),
            command_capture_sink,
        }
    }
}

impl PtyProxy {
    pub fn run(self, command_capture_sink: Option<CommandCaptureSink>) {
        match self.cmd {
            Some(Cmd::Init(init)) => {
                if let Err(err) = init.run() {
                    eprintln!("atuin pty-proxy: {err}");
                    std::process::exit(1);
                }
            }
            None => runtime::main(RuntimeOptions::new(self.debug_osc133, command_capture_sink)),
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

fn env_flag(name: &str) -> bool {
    std::env::var(name).is_ok_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

fn render_init(shell: Shell) -> &'static str {
    match shell {
        Shell::Bash | Shell::Zsh => {
            r#"if [[ "$-" == *i* ]] && [[ -t 0 ]] && [[ -t 1 ]]; then
  _atuin_pty_proxy_tmux_current="${TMUX:-}"
  _atuin_pty_proxy_tmux_previous="${ATUIN_PTY_PROXY_TMUX:-}"

  if [[ -z "${ATUIN_PTY_PROXY_ACTIVE:-}" ]] || [[ "$_atuin_pty_proxy_tmux_current" != "$_atuin_pty_proxy_tmux_previous" ]]; then
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
    end

    if not set -q ATUIN_PTY_PROXY_ACTIVE
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
    let tmux_previous = ($env.ATUIN_PTY_PROXY_TMUX? | default "")

    if (($env.ATUIN_PTY_PROXY_ACTIVE? | default "") | is-empty) or ($tmux_current != $tmux_previous) {
        $env.ATUIN_PTY_PROXY_ACTIVE = "1"
        $env.ATUIN_PTY_PROXY_TMUX = $tmux_current
        exec atuin pty-proxy
    }
}
"#
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
