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
}

impl RuntimeOptions {
    fn new(
        debug_osc133: bool,
        shell: Option<PathBuf>,
        command_capture_sink: Option<CommandCaptureSink>,
    ) -> Self {
        Self {
            debug_osc133: debug_osc133 || env_flag("ATUIN_PTY_PROXY_DEBUG"),
            shell,
            command_capture_sink,
        }
    }
}

impl PtyProxy {
    pub fn run(self, command_capture_sink: Option<CommandCaptureSink>) {
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
                command_capture_sink,
            )),
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
    // Each shell embeds its own interpreter path in the `--shell` argument so
    // `atuin pty-proxy` spawns the same binary that sourced the init, rather
    // than resolving via $PATH (which can pick the wrong installation when the
    // user has, for instance, both /usr/bin/bash and /opt/homebrew/bin/bash).
    match shell {
        Shell::Bash | Shell::Zsh => {
            r#"if [[ "$-" == *i* ]] && [[ -t 0 ]] && [[ -t 1 ]]; then
  _atuin_pty_proxy_tmux_current="${TMUX:-}"
  _atuin_pty_proxy_tmux_previous="${ATUIN_PTY_PROXY_TMUX:-}"
  _atuin_pty_proxy_tty_current="$(command tty 2>/dev/null || true)"
  _atuin_pty_proxy_tty_previous="${ATUIN_PTY_PROXY_TTY:-}"

  _atuin_pty_proxy_start=""
  if [[ -z "${ATUIN_PTY_PROXY_ACTIVE:-}" ]]; then
    _atuin_pty_proxy_start=1
  elif [[ "$_atuin_pty_proxy_tmux_current" != "$_atuin_pty_proxy_tmux_previous" ]]; then
    _atuin_pty_proxy_start=1
  elif [[ -n "$_atuin_pty_proxy_tty_previous" ]] && [[ -n "$_atuin_pty_proxy_tty_current" ]] && [[ "$_atuin_pty_proxy_tty_current" != "$_atuin_pty_proxy_tty_previous" ]]; then
    # The shell runs on a different terminal surface (e.g. a multiplexer
    # pane) than the proxy that exported these variables, so its screen
    # state does not describe this surface. Start a proxy of our own.
    _atuin_pty_proxy_start=1
  fi

  if [[ -n "$_atuin_pty_proxy_start" ]]; then
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

  unset _atuin_pty_proxy_tmux_current _atuin_pty_proxy_tmux_previous _atuin_pty_proxy_tty_current _atuin_pty_proxy_tty_previous _atuin_pty_proxy_start
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

    set -l _atuin_pty_proxy_tty_current (command tty 2>/dev/null)
    set -l _atuin_pty_proxy_tty_previous ""
    if set -q ATUIN_PTY_PROXY_TTY
        set _atuin_pty_proxy_tty_previous "$ATUIN_PTY_PROXY_TTY"
    end

    set -l _atuin_pty_proxy_start 0
    if not set -q ATUIN_PTY_PROXY_ACTIVE
        set _atuin_pty_proxy_start 1
    else if test "$_atuin_pty_proxy_tmux_current" != "$_atuin_pty_proxy_tmux_previous"
        set _atuin_pty_proxy_start 1
    else if test -n "$_atuin_pty_proxy_tty_previous"; and test -n "$_atuin_pty_proxy_tty_current"; and test "$_atuin_pty_proxy_tty_current" != "$_atuin_pty_proxy_tty_previous"
        # The shell runs on a different terminal surface (e.g. a multiplexer
        # pane) than the proxy that exported these variables, so its screen
        # state does not describe this surface. Start a proxy of our own.
        set _atuin_pty_proxy_start 1
    end

    if test $_atuin_pty_proxy_start -eq 1
        set -gx ATUIN_PTY_PROXY_ACTIVE 1
        set -gx ATUIN_PTY_PROXY_TMUX "$_atuin_pty_proxy_tmux_current"
        exec atuin pty-proxy --shell (status fish-path)
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
    let tty_current = (try { ^tty | str trim } catch { "" })
    let tty_previous = ($env.ATUIN_PTY_PROXY_TTY? | default "")

    # A non-empty tty_previous that differs from tty_current means the shell
    # runs on a different terminal surface (e.g. a multiplexer pane) than the
    # proxy that exported these variables, so its screen state does not
    # describe this surface. Start a proxy of our own.
    let tty_changed = (not ($tty_previous | is-empty)) and (not ($tty_current | is-empty)) and ($tty_current != $tty_previous)

    if (($env.ATUIN_PTY_PROXY_ACTIVE? | default "") | is-empty) or ($tmux_current != $tmux_previous) or $tty_changed {
        $env.ATUIN_PTY_PROXY_ACTIVE = "1"
        $env.ATUIN_PTY_PROXY_TMUX = $tmux_current
        exec atuin pty-proxy --shell $nu.current-exe
    }
}
"#
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Shell, render_init, shell_from_name};

    /// How `ATUIN_PTY_PROXY_TTY` should look to the shell under test.
    enum ProxyTty {
        /// The PTY the shell actually runs on — a proxy owning this surface.
        OwnDevice,
        /// A different device — env inherited across a multiplexer boundary.
        Foreign,
        /// Not set — environment from a proxy that predates the variable.
        Unset,
    }

    /// Run the rendered bash init guard inside a real PTY, with `exec atuin
    /// pty-proxy` replaced by an echo marker, and return everything the
    /// shell printed.
    fn run_bash_guard(active: bool, proxy_tty: &ProxyTty) -> String {
        use std::io::Read;

        use portable_pty::{CommandBuilder, PtySize, native_pty_system};

        let script = render_init(Shell::Bash).replace(
            "exec atuin pty-proxy",
            "echo WOULD_EXEC_PTY_PROXY # atuin pty-proxy",
        );
        let script_path = std::env::temp_dir().join(format!(
            "atuin-pty-proxy-guard-test-{}-{active}-{}.sh",
            std::process::id(),
            match proxy_tty {
                ProxyTty::OwnDevice => "own",
                ProxyTty::Foreign => "foreign",
                ProxyTty::Unset => "unset",
            },
        ));
        std::fs::write(&script_path, script).expect("write init script");

        let pty = native_pty_system();
        let pair = pty
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("openpty");
        let slave_device = pair.master.tty_name().expect("pty slave device name");

        let mut cmd = CommandBuilder::new("bash");
        let source_line = format!("source '{}'; echo GUARD_RAN", script_path.display());
        cmd.args(["--noprofile", "--norc", "-i", "-c", &source_line]);
        // Pin every variable the guard reads: the test process may itself be
        // running under tmux or an atuin pty-proxy, and the base environment
        // is inherited.
        cmd.env("TMUX", "");
        cmd.env("ATUIN_PTY_PROXY_TMUX", "");
        if active {
            cmd.env("ATUIN_PTY_PROXY_ACTIVE", "1");
        } else {
            cmd.env_remove("ATUIN_PTY_PROXY_ACTIVE");
        }
        match proxy_tty {
            ProxyTty::OwnDevice => cmd.env("ATUIN_PTY_PROXY_TTY", &slave_device),
            ProxyTty::Foreign => cmd.env("ATUIN_PTY_PROXY_TTY", "/dev/atuin-test-elsewhere"),
            ProxyTty::Unset => cmd.env_remove("ATUIN_PTY_PROXY_TTY"),
        }

        let mut child = pair.slave.spawn_command(cmd).expect("spawn bash");
        drop(pair.slave);

        let mut reader = pair.master.try_clone_reader().expect("clone reader");
        let output_thread = std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = reader.read_to_end(&mut buf);
            String::from_utf8_lossy(&buf).into_owned()
        });

        child.wait().expect("wait for bash");
        let output = output_thread.join().expect("join reader thread");
        let _ = std::fs::remove_file(&script_path);
        output
    }

    #[test]
    fn bash_guard_starts_proxy_when_inactive() {
        let output = run_bash_guard(false, &ProxyTty::Unset);
        assert!(output.contains("WOULD_EXEC_PTY_PROXY"), "output: {output}");
    }

    #[test]
    fn bash_guard_skips_proxy_on_own_device() {
        let output = run_bash_guard(true, &ProxyTty::OwnDevice);
        assert!(!output.contains("WOULD_EXEC_PTY_PROXY"), "output: {output}");
        assert!(output.contains("GUARD_RAN"), "output: {output}");
    }

    #[test]
    fn bash_guard_starts_proxy_on_foreign_device() {
        // A multiplexer pane (e.g. herdr) inherits ATUIN_PTY_PROXY_ACTIVE and
        // ATUIN_PTY_PROXY_TTY from a proxy wrapping an outer shell, but runs
        // on its own PTY — the guard must start a proxy for this surface.
        let output = run_bash_guard(true, &ProxyTty::Foreign);
        assert!(output.contains("WOULD_EXEC_PTY_PROXY"), "output: {output}");
    }

    #[test]
    fn bash_guard_skips_proxy_when_tty_unknown() {
        // Environment from a proxy that could not name its slave device (or
        // an older atuin): fall back to the previous behavior of trusting
        // ATUIN_PTY_PROXY_ACTIVE rather than exec-looping.
        let output = run_bash_guard(true, &ProxyTty::Unset);
        assert!(!output.contains("WOULD_EXEC_PTY_PROXY"), "output: {output}");
        assert!(output.contains("GUARD_RAN"), "output: {output}");
    }

    #[test]
    fn init_scripts_guard_on_tty_change() {
        let posix = render_init(Shell::Bash);
        assert!(
            posix.contains(r#"_atuin_pty_proxy_tty_current="$(command tty 2>/dev/null || true)""#)
        );
        assert!(posix.contains(r#"_atuin_pty_proxy_tty_previous="${ATUIN_PTY_PROXY_TTY:-}""#));

        let fish = render_init(Shell::Fish);
        assert!(fish.contains("ATUIN_PTY_PROXY_TTY"));
        assert!(fish.contains("command tty"));

        let nu = render_init(Shell::Nu);
        assert!(nu.contains("ATUIN_PTY_PROXY_TTY"));
        assert!(nu.contains("^tty"));
    }

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
