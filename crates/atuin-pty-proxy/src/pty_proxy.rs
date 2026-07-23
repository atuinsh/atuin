use std::path::PathBuf;

use clap::{Args, Subcommand, ValueEnum};

use crate::{CommandCaptureSink, runtime};

#[derive(Args, Debug)]
pub struct PtyProxy {
    /// Highlight OSC 133 prompt, input, output, and exit-code regions
    #[arg(long)]
    debug_osc133: bool,

    /// Path to the shell binary that atuin pty-proxy should spawn.
    /// Defaults to the system login shell. Only valid when no subcommand is given.
    #[arg(long, value_name = "PATH")]
    shell: Option<PathBuf>,

    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// Print shell code to initialize atuin pty-proxy on shell startup
    Init(Init),
    /// Emit an OSC 7 sequence describing the current working directory.
    ///
    /// Designed to be called from per-prompt hooks in pty-proxy's init
    /// scripts; the proxy parses the emitted sequence and `chdir`s itself so
    /// that terminals and multiplexers reading cwd via process introspection
    /// see the inner shell's cwd instead of the proxy's startup directory.
    EmitOsc7,
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
    pub(crate) shell: Option<PathBuf>,
    pub(crate) command_capture_sink: Option<CommandCaptureSink>,
    pub(crate) child_umask: Option<u32>,
}

impl RuntimeOptions {
    fn new(
        debug_osc133: bool,
        shell: Option<PathBuf>,
        command_capture_sink: Option<CommandCaptureSink>,
        child_umask: Option<u32>,
    ) -> Self {
        Self {
            debug_osc133: debug_osc133 || env_flag("ATUIN_PTY_PROXY_DEBUG"),
            shell,
            command_capture_sink,
            child_umask,
        }
    }
}

impl PtyProxy {
    /// `child_umask` is the umask to restore in the spawned shell. Atuin sets
    /// a restrictive process-wide umask early in startup, which the shell
    /// would otherwise inherit (#3695).
    pub fn run(self, command_capture_sink: Option<CommandCaptureSink>, child_umask: Option<u32>) {
        if self.cmd.is_some() && self.shell.is_some() {
            eprintln!("atuin pty-proxy: --shell only applies when no subcommand is given");
            std::process::exit(2);
        }
        match self.cmd {
            Some(Cmd::Init(init)) => {
                if let Err(err) = init.run() {
                    eprintln!("atuin pty-proxy: {err}");
                    std::process::exit(1);
                }
            }
            Some(Cmd::EmitOsc7) => {
                if let Err(err) = emit_osc7() {
                    eprintln!("atuin pty-proxy: {err}");
                    std::process::exit(1);
                }
            }
            None => runtime::main(RuntimeOptions::new(
                self.debug_osc133,
                self.shell,
                command_capture_sink,
                child_umask,
            )),
        }
    }
}

/// Print `\e]7;file://<encoded-cwd>\e\\` for the current working directory.
fn emit_osc7() -> Result<(), String> {
    let cwd = std::env::current_dir().map_err(|e| format!("getcwd failed: {e}"))?;
    let bytes = cwd.into_os_string().into_encoded_bytes();
    let encoded = percent_encoding::percent_encode(&bytes, crate::osc7::PATH_ENCODE_SET);
    print!("\x1b]7;file://{encoded}\x1b\\");
    Ok(())
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
    // Each shell embeds its own interpreter path in the `--shell` argument so
    // `atuin pty-proxy` spawns the same binary that sourced the init, rather
    // than resolving via $PATH (which can pick the wrong installation when the
    // user has, for instance, both /usr/bin/bash and /opt/homebrew/bin/bash).
    match shell {
        Shell::Bash | Shell::Zsh => {
            r#"if [[ "$-" == *i* ]] && [[ -t 0 ]] && [[ -t 1 ]]; then
  _atuin_pty_proxy_tmux_current="${TMUX:-}"
  _atuin_pty_proxy_tmux_previous="${ATUIN_PTY_PROXY_TMUX:-}"

  if [[ -z "${ATUIN_PTY_PROXY_ACTIVE:-}" ]] || [[ "$_atuin_pty_proxy_tmux_current" != "$_atuin_pty_proxy_tmux_previous" ]]; then
    export ATUIN_PTY_PROXY_ACTIVE=1
    export ATUIN_PTY_PROXY_TMUX="$_atuin_pty_proxy_tmux_current"
    if [[ -n "${BASH_VERSION:-}" ]]; then
      exec atuin pty-proxy --shell "$BASH"
    elif [[ -n "${ZSH_VERSION:-}" ]]; then
      # Prefer ZSH_ARGZERO (zsh 5.3+) — it preserves the path zsh was
      # invoked with — and fall back to PATH lookup otherwise. Login shells
      # set argv[0] to "-zsh", and ZSH_ARGZERO keeps that leading dash, so
      # strip it (${var#-}) before passing the path along.
      _atuin_pty_proxy_zsh="${ZSH_ARGZERO:-$(command -v zsh)}"
      exec atuin pty-proxy --shell "${_atuin_pty_proxy_zsh#-}"
    else
      exec atuin pty-proxy
    fi
  fi

  unset _atuin_pty_proxy_tmux_current _atuin_pty_proxy_tmux_previous
fi

# When running under atuin pty-proxy, emit OSC 7 (cwd) when $PWD changes so
# the proxy can mirror our cwd.  All path encoding lives in `atuin pty-proxy
# emit-osc7` (Rust) — there is no shell-side encoder.  We skip emission on
# unchanged PWD (most prompts don't follow a `cd`) and detach the
# subprocess so it doesn't block prompt rendering.
if [[ -n "${ATUIN_PTY_PROXY_SOCKET:-}" ]]; then
  _atuin_pty_proxy_osc7() {
    if [[ "$PWD" != "${_atuin_pty_proxy_last_pwd:-}" ]]; then
      _atuin_pty_proxy_last_pwd="$PWD"
      (atuin pty-proxy emit-osc7 &)
    fi
  }
  if [[ -n "${BASH_VERSION:-}" ]]; then
    if [[ "${PROMPT_COMMAND:-}" != *_atuin_pty_proxy_osc7* ]]; then
      PROMPT_COMMAND="_atuin_pty_proxy_osc7${PROMPT_COMMAND:+;$PROMPT_COMMAND}"
    fi
  elif [[ -n "${ZSH_VERSION:-}" ]]; then
    autoload -Uz add-zsh-hook
    add-zsh-hook precmd _atuin_pty_proxy_osc7
  fi
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
        exec atuin pty-proxy --shell (status fish-path)
    else if test "$_atuin_pty_proxy_tmux_current" != "$_atuin_pty_proxy_tmux_previous"
        set -gx ATUIN_PTY_PROXY_ACTIVE 1
        set -gx ATUIN_PTY_PROXY_TMUX "$_atuin_pty_proxy_tmux_current"
        exec atuin pty-proxy --shell (status fish-path)
    end
end

# When running under atuin pty-proxy, emit OSC 7 (cwd) when $PWD changes so
# the proxy can mirror our cwd.  All path encoding lives in `atuin pty-proxy
# emit-osc7` (Rust) — there is no shell-side encoder.  --on-variable PWD
# fires only on cd (not every prompt); the subprocess is detached so it
# doesn't block.
if set -q ATUIN_PTY_PROXY_SOCKET
    function __atuin_pty_proxy_osc7 --on-variable PWD
        command atuin pty-proxy emit-osc7 &
    end
    # Initial emission so the proxy knows our cwd before the first cd.
    command atuin pty-proxy emit-osc7 &
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
        exec atuin pty-proxy --shell $nu.current-exe
    }
}

# When running under atuin pty-proxy, emit OSC 7 (cwd) when $env.PWD changes
# so the proxy can mirror our cwd.  All path encoding lives in `atuin
# pty-proxy emit-osc7` (Rust) — there is no shell-side encoder.  The
# env_change.PWD hook fires only on cd (not every prompt); `job spawn` runs
# the subprocess asynchronously so it doesn't block.
if not ($env.ATUIN_PTY_PROXY_SOCKET? | default "" | is-empty) {
    let _atuin_pty_proxy_osc7 = {|_before, _after|
        job spawn { ^atuin pty-proxy emit-osc7 } | ignore
    }
    $env.config.hooks.env_change.PWD = (
        ($env.config.hooks.env_change.PWD? | default []) | append $_atuin_pty_proxy_osc7
    )
    # Nushell auto-fires env_change.PWD on shell startup
    # (before="" -> after=$PWD), so the proxy learns our cwd without a
    # manual initial fire here.
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
    fn init_scripts_forward_shell_path() {
        let posix = render_init(Shell::Bash);
        assert!(posix.contains(r#"exec atuin pty-proxy --shell "$BASH""#));
        // zsh: capture ZSH_ARGZERO (with PATH fallback), then strip the
        // leading dash present on login shells before forwarding the path.
        assert!(posix.contains(r#"_atuin_pty_proxy_zsh="${ZSH_ARGZERO:-$(command -v zsh)}""#));
        assert!(posix.contains(r#"exec atuin pty-proxy --shell "${_atuin_pty_proxy_zsh#-}""#));

        let fish = render_init(Shell::Fish);
        assert!(fish.contains("exec atuin pty-proxy --shell (status fish-path)"));

        let nu = render_init(Shell::Nu);
        assert!(nu.contains("exec atuin pty-proxy --shell $nu.current-exe"));
    }
    #[test]
    fn init_scripts_emit_osc7_on_pwd_change() {
        // Each shell wires a PWD hook that shells out to `emit-osc7`, gated on
        // the proxy socket being present.
        let posix = render_init(Shell::Bash);
        assert!(posix.contains("ATUIN_PTY_PROXY_SOCKET"));
        assert!(posix.contains("atuin pty-proxy emit-osc7"));
        assert!(posix.contains("add-zsh-hook precmd _atuin_pty_proxy_osc7"));

        let fish = render_init(Shell::Fish);
        assert!(fish.contains("ATUIN_PTY_PROXY_SOCKET"));
        assert!(fish.contains("--on-variable PWD"));
        assert!(fish.contains("atuin pty-proxy emit-osc7"));

        let nu = render_init(Shell::Nu);
        assert!(nu.contains("ATUIN_PTY_PROXY_SOCKET"));
        assert!(nu.contains("env_change.PWD"));
        assert!(nu.contains("atuin pty-proxy emit-osc7"));
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
