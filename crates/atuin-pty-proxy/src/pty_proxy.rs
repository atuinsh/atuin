use std::ffi::OsString;

use clap::{Args, Subcommand, ValueEnum};

use crate::{CaptureConfig, runtime};

#[derive(Args, Debug)]
pub struct PtyProxy {
    /// Highlight `OSC 18188735` escape sequences used to mark the start and end of command output.
    #[arg(long)]
    debug_markers: bool,

    /// Path to the shell binary that atuin pty-proxy should spawn.
    ///
    /// Defaults to the system login shell. Only valid when no subcommand is given.
    ///
    /// Pass multiple times to provide arguments to the shell.
    #[arg(long, value_name = "PATH", allow_hyphen_values = true)]
    shell: Vec<OsString>,

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
    pub(crate) debug_markers: bool,
    pub(crate) shell: Vec<OsString>,
    pub(crate) command_capture: Box<dyn FnOnce() -> Option<CaptureConfig>>,
    pub(crate) child_umask: Option<u32>,
}

impl RuntimeOptions {
    fn new(
        debug_markers: bool,
        shell: Vec<OsString>,
        command_capture: Box<dyn FnOnce() -> Option<CaptureConfig>>,
        child_umask: Option<u32>,
    ) -> Self {
        Self {
            debug_markers: debug_markers || env_flag("ATUIN_PTY_PROXY_DEBUG"),
            shell,
            command_capture,
            child_umask,
        }
    }
}

impl PtyProxy {
    /// `command_capture` is called on the proxy's runtime, so it can spawn the task that drains
    /// what its sink is handed.
    ///
    /// `child_umask` is the umask to restore in the spawned shell. Atuin sets
    /// a restrictive process-wide umask early in startup, which the shell
    /// would otherwise inherit (#3695).
    pub fn run(
        self,
        command_capture: impl FnOnce() -> Option<CaptureConfig> + 'static,
        child_umask: Option<u32>,
    ) {
        if self.cmd.is_some() && !self.shell.is_empty() {
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
                self.debug_markers,
                self.shell,
                Box::new(command_capture),
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

    if let Ok(shell) = atuin_common::env::var("ATUIN_SHELL")
        && let Some(shell) = shell_from_name(&shell)
    {
        return Ok(shell);
    }

    if let Ok(shell) = atuin_common::env::var("SHELL")
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
    atuin_common::env::var(name).is_ok_and(|value| {
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
const BASH_ZSH_INIT: &str = r#"if [[ "$-" == *i* ]] && [[ -t 0 ]] && [[ -t 1 ]]; then
  if [[ -z ${__atuin_pty_proxy_owns_tty-} ]]; then
    if __atuin_pty_proxy_answer=$(atuin __internal pty-proxy-active 2>/dev/null) &&
      [[ $__atuin_pty_proxy_answer = 1 ]]
    then
      __atuin_pty_proxy_owns_tty=1
    else
      __atuin_pty_proxy_owns_tty=0
    fi
  fi

  # Check if we're allowed to spawn a PTY proxy in this shell. If false, this is most commonly
  # because the shell was started with `-ic <command>` or `-i <file>`, and spawning a proxy would
  # fail to forward those arguments.
  __atuin_pty_proxy_spawning_allowed() {
    if [[ -n ${ZSH_VERSION-} ]]; then
      # If `-c` was passed, return false.
      [[ -n ${ZSH_EXECUTION_STRING+x} ]] && return 1

      # If missing ZSH_ARGZERO, default to true.
      [[ -z ${ZSH_ARGZERO-} ]] && return 0

      # If ZSH_ARGZERO starts with "-" but the shell isn't a login shell, we're executing
      # a file, so return false.
      [[ $ZSH_ARGZERO == -* ]] && ! [[ -o login ]] && return 1

      # Get basename of ZSH_ARGZERO and strip leading "-" (matches how ZSH_NAME is calculated)
      # and compare to ZSH_NAME. If they're different, we're executing a file, so return false.
      [[ ${${ZSH_ARGZERO:t}#-} != "$ZSH_NAME" ]] && return 1

      # Finally, return true iff $ZSH_ARGZERO (minus leading "-") is an executable. This
      # is what we use when spawning the proxy, so we would necessarily fail otherwise.
      local zsh="${${ZSH_ARGZERO#-}:c}"
      [[ $zsh == */* ]] && [[ -x $zsh ]] && ! [[ -d $zsh ]]
    elif [[ -n ${BASH_VERSION-} ]]; then
      # If `-c` was passed, return false.
      [[ $- == *c* ]] && return 1

      # If $0 starts with "-" but the shell isn't a login shell, we're executing a file,
      # so return false.
      [[ $0 == -* ]] && ! shopt -q login_shell && return 1

      # Finally, return true iff $0 (minus leading "-") is the same file as $BASH or matches
      # the basename of $BASH.
      [[ ${0#-} -ef $BASH ]] || [[ ${0#-} == "${BASH##*/}" ]]
    else
      true
    fi
  }

  if [[ $__atuin_pty_proxy_owns_tty = 1 ]]; then
    # This terminal is already running in an Atuin PTY proxy.
    :
  elif [[ -n ${ATUIN_PTY_PROXY_FAILED-} ]]; then
    # If the PTY proxy failed to spawn its server, stop here instead of endlessly
    # trying to spawn more proxies.
    :
  elif ! __atuin_pty_proxy_spawning_allowed; then
    :
  elif [[ -n "${BASH_VERSION:-}" ]]; then
    __atuin_pty_proxy_args=(--shell "$BASH")
    if shopt -q login_shell; then
      __atuin_pty_proxy_args+=(--shell -l)
    fi
    exec atuin pty-proxy "${__atuin_pty_proxy_args[@]}"
  elif [[ -n "${ZSH_VERSION:-}" ]]; then
    # Prefer ZSH_ARGZERO (zsh 5.3+) -- it preserves the path zsh was
    # invoked with -- and fall back to PATH lookup otherwise. Login shells
    # set argv[0] to "-zsh", and ZSH_ARGZERO keeps that leading dash, so
    # strip it (${var#-}) before passing it along. ZSH_ARGZERO may also be
    # a bare command name ("zsh") rather than a path; the :c modifier
    # resolves that to an absolute path via $PATH, leaves an absolute path
    # unchanged, and leaves an unresolvable name as-is.
    if [[ -n ${ZSH_ARGZERO-} ]]; then
      __atuin_pty_proxy_zsh=${${ZSH_ARGZERO#-}:c}
    else
      __atuin_pty_proxy_zsh=$(command -v zsh)
    fi
    __atuin_pty_proxy_args=(--shell "$__atuin_pty_proxy_zsh")
    if [[ -o login ]]; then
      __atuin_pty_proxy_args+=(--shell -l)
    fi
    exec atuin pty-proxy "${__atuin_pty_proxy_args[@]}"
  else
    exec atuin pty-proxy
  fi

  unset -v __atuin_pty_proxy_answer
  unset -f __atuin_pty_proxy_spawning_allowed
fi
"#;

/// Preamble for fish.
// Unlike other shells, we only test whether stdout is a tty rather than also checking stdin,
// because we instruct users to pipe `atuin init fish` to `source`, which puts the script itself
// on stdin, making it necessarily not a tty. This is also why we do `</dev/tty` when invoking
// `atuin pty-proxy` if stdin is not a terminal.
const FISH_INIT: &str = r#"if status is-interactive; and test -t 1
    if not set -q __atuin_pty_proxy_owns_tty
        set -g __atuin_pty_proxy_owns_tty 0
        set -l __atuin_pty_proxy_answer (atuin __internal pty-proxy-active 2>/dev/null)
        if test $status -eq 0; and test "$__atuin_pty_proxy_answer" = 1
            set -g __atuin_pty_proxy_owns_tty 1
        else
            set -g __atuin_pty_proxy_owns_tty 0
        end
    end

    if test "$__atuin_pty_proxy_owns_tty" != 1; and not set -q ATUIN_PTY_PROXY_FAILED
        set __atuin_pty_proxy_args --shell (status fish-path)
        if status is-login
            set -a __atuin_pty_proxy_args --shell -l
        end
        if test -t 0
            exec atuin pty-proxy $__atuin_pty_proxy_args
        else
            exec atuin pty-proxy $__atuin_pty_proxy_args </dev/tty
        end
    end
end
"#;

/// Preamble for Nushell.
///
/// Nushell cannot dynamically source the output of `atuin init nu`, so only the pty-proxy
/// preamble is emitted here; users must also set up `atuin init nu` separately.
const NU_INIT: &str = r#"if (is-terminal --stdin) and (is-terminal --stdout) {
    if '__atuin_pty_proxy' not-in $env {
        let atuin_proxy_check = (do -i { atuin __internal pty-proxy-active } | complete)

        # Use a record rather than a plain string so the variable doesn't get exported
        # to child processes -- we want each child shell to perform its own detection.
        $env.__atuin_pty_proxy = { owns_tty: (
            $atuin_proxy_check.exit_code == 0 and ($atuin_proxy_check.stdout | str trim) == "1"
        ) }
    }

    if not $env.__atuin_pty_proxy.owns_tty and 'ATUIN_PTY_PROXY_FAILED' not-in $env {
        mut pty_proxy_args = [--shell $nu.current-exe]
        if $nu.is-login {
            $pty_proxy_args = ($pty_proxy_args ++ [--shell -l])
        }
        exec atuin pty-proxy ...$pty_proxy_args
    }
}
"#;

#[cfg(test)]
mod tests {
    use regex::Regex;
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
        assert!(posix.contains(r#"__atuin_pty_proxy_args=(--shell "$BASH")"#));
        // zsh: capture ZSH_ARGZERO (with PATH fallback), strip the leading
        // dash present on login shells, then resolve a bare command name to
        // an absolute path with the :c modifier before forwarding it.
        assert!(posix.contains(r"__atuin_pty_proxy_zsh=${${ZSH_ARGZERO#-}:c}"));
        assert!(posix.contains(r"__atuin_pty_proxy_zsh=$(command -v zsh)"));
        assert!(posix.contains(r#"__atuin_pty_proxy_args=(--shell "$__atuin_pty_proxy_zsh")"#));

        let fish = init_script(Shell::Fish);
        assert!(fish.contains("set __atuin_pty_proxy_args --shell (status fish-path)"));

        let nu = init_script(Shell::Nu);
        assert!(nu.contains("mut pty_proxy_args = [--shell $nu.current-exe]"));
    }

    #[rstest]
    #[case::bash(Shell::Bash, "[[ $- == *c* ]]")]
    #[case::zsh(Shell::Zsh, "[[ -n ${ZSH_EXECUTION_STRING+x} ]]")]
    fn bash_zsh_init_skips_shells_started_with_a_command(
        #[case] shell: Shell,
        #[case] expected: &str,
    ) {
        // `bash -ic cmd` and `zsh -ic cmd` are interactive, but re-execing them would drop `cmd`.
        assert!(init_script(shell).contains(expected));
    }

    #[rstest]
    fn fish_init_execs_the_proxy_with_the_terminal_on_stdin() {
        // When a script is piped into `source` in fish, stdin is not a terminal. `atuin pty-proxy`
        // expects stdin to be a terminal, so we need to make sure we explicitly connect it to one.
        let exec = r"exec atuin pty-proxy \$__atuin_pty_proxy_args";
        let branch =
            Regex::new(&format!(r"if test -t 0\s+{exec}\s+else\s+{exec} </dev/tty\s+end")).unwrap();

        assert!(
            branch.is_match(init_script(Shell::Fish)),
            "fish init: {}",
            init_script(Shell::Fish)
        );
    }

    #[rstest]
    #[case::bash(
        Shell::Bash,
        r"if shopt -q login_shell; then\s+__atuin_pty_proxy_args\+=\(--shell -l\)"
    )]
    #[case::zsh(
        Shell::Zsh,
        r"if \[\[ -o login \]\]; then\s+__atuin_pty_proxy_args\+=\(--shell -l\)"
    )]
    #[case::fish(Shell::Fish, r"if status is-login\s+set -a __atuin_pty_proxy_args --shell -l")]
    #[case::nu(
        Shell::Nu,
        r"if \$nu\.is-login \{\s+\$pty_proxy_args = \(\$pty_proxy_args \+\+ \[--shell -l\]\)"
    )]
    fn init_scripts_maintain_login_status(#[case] shell: Shell, #[case] expected: &str) {
        assert!(Regex::new(expected).unwrap().is_match(init_script(shell)));
    }
}
