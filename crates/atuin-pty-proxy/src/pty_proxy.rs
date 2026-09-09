use std::path::PathBuf;

use clap::{Args, Subcommand, ValueEnum};

use crate::{CaptureConfig, runtime};

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
pub enum Shell {
    /// Zsh setup
    Zsh,
    /// Bash setup
    Bash,
    /// Fish setup
    Fish,
    /// Nu setup
    Nu,
}

pub struct RuntimeOptions {
    pub(crate) debug_osc133: bool,
    pub(crate) shell: Option<PathBuf>,
    pub(crate) command_capture: Option<CaptureConfig>,
    pub(crate) child_umask: Option<u32>,
}

impl RuntimeOptions {
    fn new(
        debug_osc133: bool,
        shell: Option<PathBuf>,
        command_capture: Option<CaptureConfig>,
        child_umask: Option<u32>,
    ) -> Self {
        Self {
            debug_osc133: debug_osc133 || env_flag("ATUIN_PTY_PROXY_DEBUG"),
            shell,
            command_capture,
            child_umask,
        }
    }
}

impl PtyProxy {
    /// `child_umask` is the umask to restore in the spawned shell. Atuin sets
    /// a restrictive process-wide umask early in startup, which the shell
    /// would otherwise inherit (#3695).
    pub fn run(self, command_capture: Option<CaptureConfig>, child_umask: Option<u32>) {
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
            None => runtime::main(RuntimeOptions::new(
                self.debug_osc133,
                self.shell,
                command_capture,
                child_umask,
            )),
        }
    }
}

impl Init {
    fn run(self) -> Result<(), String> {
        let shell = detect_shell(self.shell)?;
        let script = init_script(shell);
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

    Err("could not detect a supported shell. Please specify one explicitly: bash, zsh, fish, or nu"
        .to_string())
}

fn shell_from_name(name: &str) -> Option<Shell> {
    let shell =
        name.trim().rsplit('/').next().unwrap_or(name).trim_start_matches('-').to_ascii_lowercase();

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
        matches!(value.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on")
    })
}

/// Shell code that re-execs the current shell inside `atuin pty-proxy`.
///
/// Safe to emit more than once: the script checks whether a PTY proxy has already been spawned and
/// won't spawn another. This lets `atuin init` embed the preamble (via the `pty_proxy.enabled`
/// setting) without conflicting with an existing standalone `atuin pty-proxy init` line in shell
/// config.
#[must_use]
pub fn init_script(shell: Shell) -> &'static str {
    match shell {
        Shell::Bash | Shell::Zsh => BASH_ZSH_INIT,
        Shell::Fish => FISH_INIT,
        Shell::Nu => NU_INIT,
    }
}

/// Preamble for Bash and Zsh.
///
/// Each shell embeds its own interpreter path in the `--shell` argument so `atuin pty-proxy`
/// spawns the same binary that sourced the init, rather than resolving via `$PATH` (which can
/// pick the wrong installation when the user has, for instance, both `/usr/bin/bash` and
/// `/opt/homebrew/bin/bash`).
const BASH_ZSH_INIT: &str = r#"if [[ "$-" == *i* ]] && [[ -t 0 ]] && [[ -t 1 ]] &&
  [[ -z ${__atuin_pty_proxy_owns_tty-} ]]
then
  __atuin_pty_proxy_owns_tty=0

  # Check whether this terminal is already running in an Atuin PTY proxy.
  if ! __atuin_pty_proxy_answer=$(atuin __internal pty-proxy-active 2>/dev/null); then
    :
  elif [[ $__atuin_pty_proxy_answer = 1 ]]; then
    __atuin_pty_proxy_owns_tty=1
  elif [[ -n ${ATUIN_PTY_PROXY_FAILED-} ]]; then
    # If the PTY proxy failed to spawn its server, stop here instead of endlessly
    # trying to spawn more proxies.
    :
  elif [[ -n "${BASH_VERSION:-}" ]]; then
    exec atuin pty-proxy --shell "$BASH"
  elif [[ -n "${ZSH_VERSION:-}" ]]; then
    # Prefer ZSH_ARGZERO (zsh 5.3+) -- it preserves the path zsh was
    # invoked with -- and fall back to PATH lookup otherwise. Login shells
    # set argv[0] to "-zsh", and ZSH_ARGZERO keeps that leading dash, so
    # strip it (${var#-}) before passing it along.
    _atuin_pty_proxy_zsh="${ZSH_ARGZERO:-$(command -v zsh)}"
    exec atuin pty-proxy --shell "${_atuin_pty_proxy_zsh#-}"
  else
    exec atuin pty-proxy
  fi
  unset __atuin_pty_proxy_answer
fi
"#;

/// Preamble for fish.
// Unlike other shells, we only test whether stdout is a tty rather than also checking stdin,
// because we instruct users to pipe `atuin init fish` to `source`, which puts the script itself
// on stdin, making it necessarily not a tty.
const FISH_INIT: &str = r#"if status is-interactive; and test -t 1
    and not set -q __atuin_pty_proxy_owns_tty

    set -g __atuin_pty_proxy_owns_tty 0
    set -l __atuin_pty_proxy_answer (atuin __internal pty-proxy-active 2>/dev/null)
    set -l __atuin_pty_proxy_status $status

    if test $__atuin_pty_proxy_status -ne 0
    else if test "$__atuin_pty_proxy_answer" = 1
        set -g __atuin_pty_proxy_owns_tty 1
    else if not set -q ATUIN_PTY_PROXY_FAILED
        exec atuin pty-proxy --shell (status fish-path)
    end
end
"#;

/// Preamble for Nushell.
///
/// Nushell cannot dynamically source the output of `atuin init nu`, so only the pty-proxy
/// preamble is emitted here; users must also set up `atuin init nu` separately.
const NU_INIT: &str = r#"if (is-terminal --stdin) and (is-terminal --stdout) and ('__atuin_pty_proxy' not-in $env) {
    # Use a record rather than a plain string so the variable doesn't get exported
    # to child processes -- we want each child shell to perform its own detection.
    $env.__atuin_pty_proxy = { owns_tty: false }

    let atuin_proxy_check = (do -i { atuin __internal pty-proxy-active } | complete)

    if $atuin_proxy_check.exit_code != 0 {
    } else if ($atuin_proxy_check.stdout | str trim) == "1" {
        $env.__atuin_pty_proxy = { owns_tty: true }
    } else if ('ATUIN_PTY_PROXY_FAILED' not-in $env) {
        exec atuin pty-proxy --shell $nu.current-exe
    }
}
"#;

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::{Shell, init_script, shell_from_name};

    #[rstest]
    #[case::zsh_abs_path("/bin/zsh", Shell::Zsh)]
    #[case::bash_abs_path("/usr/local/bin/bash", Shell::Bash)]
    #[case::fish_bare("fish", Shell::Fish)]
    #[case::nu_bare("nu", Shell::Nu)]
    fn shell_from_name_maps(#[case] input: &str, #[case] expected: Shell) {
        assert_eq!(shell_from_name(input), Some(expected));
    }

    #[rstest]
    fn every_init_execs_pty_proxy(
        #[values(Shell::Zsh, Shell::Bash, Shell::Fish, Shell::Nu)] shell: Shell,
    ) {
        assert!(init_script(shell).contains("exec atuin pty-proxy"));
    }

    #[rstest]
    fn every_init_asks_atuin_whether_this_terminal_has_a_proxy(
        #[values(Shell::Zsh, Shell::Bash, Shell::Fish, Shell::Nu)] shell: Shell,
    ) {
        assert!(init_script(shell).contains("__internal pty-proxy-active"));
    }

    #[rstest]
    fn every_init_stops_when_the_proxy_says_it_failed(
        #[values(Shell::Zsh, Shell::Bash, Shell::Fish, Shell::Nu)] shell: Shell,
    ) {
        let script = init_script(shell);
        assert!(script.contains("ATUIN_PTY_PROXY_FAILED"));
    }

    #[rstest]
    #[case(Shell::Bash, "[[ -z ${__atuin_pty_proxy_owns_tty-} ]]")]
    #[case(Shell::Fish, "not set -q __atuin_pty_proxy_owns_tty")]
    #[case(Shell::Nu, "'__atuin_pty_proxy' not-in $env")]
    fn init_no_ops_when_emitted_twice(#[case] shell: Shell, #[case] guard: &str) {
        // `atuin init` embeds the preamble, and users may also still have a standalone
        // `atuin pty-proxy init` line. Whichever copy runs first decides; the second must not
        // repeat the check.
        assert!(init_script(shell).contains(guard), "{shell:?} repeats the check");
    }

    #[rstest]
    fn posix_init_has_no_double_braces() {
        let script = init_script(Shell::Bash);
        assert!(!script.contains("${{"), "double braces in bash init script");
    }

    #[rstest]
    fn init_scripts_forward_shell_path() {
        let posix = init_script(Shell::Bash);
        assert!(posix.contains(r#"exec atuin pty-proxy --shell "$BASH""#));
        // zsh: capture ZSH_ARGZERO (with PATH fallback), then strip the
        // leading dash present on login shells before forwarding the path.
        assert!(posix.contains(r#"_atuin_pty_proxy_zsh="${ZSH_ARGZERO:-$(command -v zsh)}""#));
        assert!(posix.contains(r#"exec atuin pty-proxy --shell "${_atuin_pty_proxy_zsh#-}""#));

        let fish = init_script(Shell::Fish);
        assert!(fish.contains("exec atuin pty-proxy --shell (status fish-path)"));

        let nu = init_script(Shell::Nu);
        assert!(nu.contains("exec atuin pty-proxy --shell $nu.current-exe"));
    }
}
