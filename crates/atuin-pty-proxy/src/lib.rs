     1|#[cfg(unix)]
     2|mod debug;
     3|pub mod osc133;
     4|#[cfg(unix)]
     5|mod runtime;
     6|#[cfg(unix)]
     7|mod screen;
     8|
     9|use clap::{Args, Subcommand, ValueEnum};
    10|
    11|#[derive(Args, Debug)]
    12|pub struct Hex {
    13|    /// Highlight OSC 133 prompt, input, output, and exit-code regions
    14|    #[arg(long)]
    15|    debug_osc133: bool,
    16|
    17|    #[command(subcommand)]
    18|    cmd: Option<Cmd>,
    19|}
    20|
    21|#[derive(Subcommand, Debug)]
    22|pub enum Cmd {
    23|    /// Print shell code to initialize atuin pty-proxy on shell startup
    24|    Init(Init),
    25|}
    26|
    27|#[derive(Args, Debug)]
    28|pub struct Init {
    29|    /// Shell to generate init for. If omitted, attempt auto-detection
    30|    #[arg(value_enum)]
    31|    shell: Option<Shell>,
    32|}
    33|
    34|#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
    35|#[value(rename_all = "lower")]
    36|#[allow(clippy::enum_variant_names, clippy::doc_markdown)]
    37|enum Shell {
    38|    /// Zsh setup
    39|    Zsh,
    40|    /// Bash setup
    41|    Bash,
    42|    /// Fish setup
    43|    Fish,
    44|    /// Nu setup
    45|    Nu,
    46|}
    47|
    48|impl Init {
    49|    fn run(self) -> Result<(), String> {
    50|        let shell = detect_shell(self.shell)?;
    51|        let script = render_init(shell);
    52|        print!("{script}");
    53|        Ok(())
    54|    }
    55|}
    56|
    57|pub fn run(hex: Hex) {
    58|    match hex.cmd {
    59|        Some(Cmd::Init(init)) => {
    60|            if let Err(err) = init.run() {
    61|                eprintln!("atuin pty-proxy: {err}");
    62|                std::process::exit(1);
    63|            }
    64|        }
    65|        None => runtime::main(RuntimeOptions::from(hex)),
    66|    }
    67|}
    68|
    69|#[derive(Clone, Copy, Debug)]
    70|pub(crate) struct RuntimeOptions {
    71|    debug_osc133: bool,
    72|}
    73|
    74|impl From<Hex> for RuntimeOptions {
    75|    fn from(hex: Hex) -> Self {
    76|        Self {
    77|            debug_osc133: hex.debug_osc133 || env_flag("ATUIN_HEX_DEBUG"),
    78|        }
    79|    }
    80|}
    81|
    82|fn env_flag(name: &str) -> bool {
    83|    std::env::var(name).is_ok_and(|value| {
    84|        matches!(
    85|            value.trim().to_ascii_lowercase().as_str(),
    86|            "1" | "true" | "yes" | "on"
    87|        )
    88|    })
    89|}
    90|
    91|fn detect_shell(cli_shell: Option<Shell>) -> Result<Shell, String> {
    92|    if let Some(shell) = cli_shell {
    93|        return Ok(shell);
    94|    }
    95|
    96|    if let Ok(shell) = std::env::var("ATUIN_SHELL")
    97|        && let Some(shell) = shell_from_name(&shell)
    98|    {
    99|        return Ok(shell);
   100|    }
   101|
   102|    if let Ok(shell) = std::env::var("SHELL")
   103|        && let Some(shell) = shell_from_name(&shell)
   104|    {
   105|        return Ok(shell);
   106|    }
   107|
   108|    Err(
   109|        "could not detect a supported shell. Please specify one explicitly: bash, zsh, fish, or nu"
   110|            .to_string(),
   111|    )
   112|}
   113|
   114|fn shell_from_name(name: &str) -> Option<Shell> {
   115|    let shell = name
   116|        .trim()
   117|        .rsplit('/')
   118|        .next()
   119|        .unwrap_or(name)
   120|        .trim_start_matches('-')
   121|        .to_ascii_lowercase();
   122|
   123|    match shell.as_str() {
   124|        "bash" => Some(Shell::Bash),
   125|        "zsh" => Some(Shell::Zsh),
   126|        "fish" => Some(Shell::Fish),
   127|        "nu" => Some(Shell::Nu),
   128|        _ => None,
   129|    }
   130|}
   131|
   132|fn render_init(shell: Shell) -> &'static str {
   133|    match shell {
   134|        Shell::Bash | Shell::Zsh => {
   135|            r#"if [[ "$-" == *i* ]] && [[ -t 0 ]] && [[ -t 1 ]]; then
   136|  _atuin_pty_proxy_tmux_current="${TMUX:-}"
   137|  _atuin_pty_proxy_tmux_previous="${ATUIN_PTY_PROXY_TMUX:-${ATUIN_HEX_TMUX:-}}"
   138|
   139|  if [[ -z "${ATUIN_PTY_PROXY_ACTIVE:-${ATUIN_HEX_ACTIVE:-}}" ]] || [[ "$_atuin_pty_proxy_tmux_current" != "$_atuin_pty_proxy_tmux_previous" ]]; then
   140|    export ATUIN_PTY_PROXY_ACTIVE=1
   141|    export ATUIN_PTY_PROXY_TMUX="$_atuin_pty_proxy_tmux_current"
   142|    exec atuin pty-proxy
   143|  fi
   144|
   145|  unset _atuin_pty_proxy_tmux_current _atuin_pty_proxy_tmux_previous
   146|fi
   147|"#
   148|        }
   149|        Shell::Fish => {
   150|            r#"if status is-interactive; and test -t 0; and test -t 1
   151|    set -l _atuin_pty_proxy_tmux_current ""
   152|    if set -q TMUX
   153|        set _atuin_pty_proxy_tmux_current "$TMUX"
   154|    end
   155|
   156|    set -l _atuin_pty_proxy_tmux_previous ""
   157|    if set -q ATUIN_PTY_PROXY_TMUX
   158|        set _atuin_pty_proxy_tmux_previous "$ATUIN_PTY_PROXY_TMUX"
   159|    else if set -q ATUIN_HEX_TMUX
   160|        set _atuin_pty_proxy_tmux_previous "$ATUIN_HEX_TMUX"
   161|    end
   162|
   163|    if not set -q ATUIN_PTY_PROXY_ACTIVE; and not set -q ATUIN_HEX_ACTIVE
   164|        set -gx ATUIN_PTY_PROXY_ACTIVE 1
   165|        set -gx ATUIN_PTY_PROXY_TMUX "$_atuin_pty_proxy_tmux_current"
   166|        exec atuin pty-proxy
   167|    else if test "$_atuin_pty_proxy_tmux_current" != "$_atuin_pty_proxy_tmux_previous"
   168|        set -gx ATUIN_PTY_PROXY_ACTIVE 1
   169|        set -gx ATUIN_PTY_PROXY_TMUX "$_atuin_pty_proxy_tmux_current"
   170|        exec atuin pty-proxy
   171|    end
   172|end
   173|"#
   174|        }
   175|        // Nushell cannot dynamically source the output of `atuin init nu`,
   176|        // so we only output the pty-proxy preamble here. Users must also set up
   177|        // `atuin init nu` separately.
   178|        Shell::Nu => {
   179|            r#"if (is-terminal --stdin) and (is-terminal --stdout) {
   180|    let tmux_current = ($env.TMUX? | default "")
   181|    let tmux_previous = ($env.ATUIN_PTY_PROXY_TMUX? | default ($env.ATUIN_HEX_TMUX? | default ""))
   182|
   183|    if (($env.ATUIN_PTY_PROXY_ACTIVE? | default ($env.ATUIN_HEX_ACTIVE? | default "")) | is-empty) or ($tmux_current != $tmux_previous) {
   184|        $env.ATUIN_PTY_PROXY_ACTIVE = "1"
   185|        $env.ATUIN_PTY_PROXY_TMUX = $tmux_current
   186|        exec atuin pty-proxy
   187|    }
   188|}
   189|"#
   190|        }
   191|    }
   192|}
   193|
   194|#[cfg(not(unix))]
   195|mod runtime {
   196|    pub(crate) fn main(_options: super::RuntimeOptions) {
   197|        eprintln!("atuin pty-proxy currently supports unix platforms");
   198|        std::process::exit(1);
   199|    }
   200|}
   201|
   905|#[cfg(test)]
   906|mod tests {
   907|    use super::{Shell, render_init, shell_from_name};
   908|
   909|    #[test]
   910|    fn shell_from_name_handles_paths() {
   911|        assert_eq!(shell_from_name("/bin/zsh"), Some(Shell::Zsh));
   912|        assert_eq!(shell_from_name("/usr/local/bin/bash"), Some(Shell::Bash));
   913|        assert_eq!(shell_from_name("fish"), Some(Shell::Fish));
   914|        assert_eq!(shell_from_name("nu"), Some(Shell::Nu));
   915|    }
   916|
   917|    #[test]
   918|    fn posix_init_uses_exec_and_tmux_guard() {
   919|        let script = render_init(Shell::Bash);
   920|        assert!(script.contains("exec atuin pty-proxy"));
   921|        assert!(script.contains("ATUIN_PTY_PROXY_TMUX"));
   922|        assert!(!script.contains("eval \"$(atuin init bash)\""));
   923|    }
   924|
   925|    #[test]
   926|    fn posix_init_has_no_double_braces() {
   927|        let script = render_init(Shell::Bash);
   928|        assert!(!script.contains("${{"), "double braces in bash init script");
   929|    }
   930|
   931|    #[test]
   932|    fn fish_init_uses_source() {
   933|        let script = render_init(Shell::Fish);
   934|        assert!(script.contains("exec atuin pty-proxy"));
   935|        assert!(!script.contains("atuin init fish | source"));
   936|    }
   937|
   938|    #[test]
   939|    fn nu_init_uses_exec_and_tty_guard() {
   940|        let script = render_init(Shell::Nu);
   941|        assert!(script.contains("exec atuin pty-proxy"));
   942|        assert!(script.contains("ATUIN_PTY_PROXY_TMUX"));
   943|        assert!(script.contains("is-terminal --stdin"));
   944|        assert!(script.contains("is-terminal --stdout"));
   945|        assert!(script.contains("ATUIN_PTY_PROXY_ACTIVE"));
   946|    }
   947|}
   948|