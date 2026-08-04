//! Headless shell-completion oracles.
//!
//! zsh's compsys only runs inside an interactive ZLE session, so a captive
//! interactive zsh lives under its own pty; bash's programmable completion
//! needs no terminal, so its captive shell runs over plain pipes; fish is
//! headless by design and runs per query. The captive init scripts print
//! candidates between NUL-delimiter lines, and one oracle persists per
//! session so rc files and compinit load once.
//!
//! The zsh `compadd` interception technique is adapted from
//! <https://github.com/Valodim/zsh-capture-completion> (MIT).

use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use portable_pty::{CommandBuilder, PtySize, native_pty_system};

const READY_MARKER: &str = "__ATUIN_ORACLE_READY__";
/// User rc files (nvm and friends) can take a while; the hermetic `-f`
/// variant has nothing to load.
const SPAWN_TIMEOUT_USER_CONFIG: Duration = Duration::from_secs(10);
const SPAWN_TIMEOUT_HERMETIC: Duration = Duration::from_secs(5);
/// The oracle thread's own per-query deadline. Generous on purpose: a slow
/// query (first-load of a completer, huge candidate sets) must finish or
/// truly wedge — abandoning it mid-protocol desyncs the NUL framing.
const QUERY_DEADLINE: Duration = Duration::from_secs(2);
/// Wedged-oracle respawns before completions are given up for the session.
const RESPAWN_LIMIT: u32 = 3;
const KILL_WHOLE_LINE: &[u8] = b"\x15";

/// Delimits completion output; re-arms itself because zsh clears the
/// pre/post function arrays after every completion.
const INIT_SCRIPT: &str = r#"
PROMPT=''
RPROMPT=''
unset zle_bracketed_paste 2>/dev/null

# The user's rc may already have run compinit; don't clobber its setup.
if ! (( ${+functions[compdef]} )); then
    autoload -Uz compinit
    compinit -C -d "${TMPDIR:-/tmp}/.atuin-oracle-zcompdump"
fi

# The oracle must never run a command, only complete.
bindkey '^M' undefined
bindkey '^J' undefined
bindkey '^I' complete-word

_atuin_pre()  { print -r -- $'\0'; compprefuncs=(_atuin_pre); }
_atuin_post() { print -r -- $'\0'; comppostfuncs=(_atuin_post); }
compprefuncs=(_atuin_pre)
comppostfuncs=(_atuin_post)

zstyle ':completion:*' list-grouped false
zstyle ':completion:*' insert-tab false
zstyle ':completion:*' list-separator ''
zstyle ':completion:*' menu no

# The user's rc may have loaded atuin's own integration (or plugins that
# invoke it per keystroke, like zsh-autosuggestions). The oracle must stay
# inert: no history hooks recording completion queries, no daemon
# autostart, no per-keystroke atuin invocations, no history file writes.
if (( ${+functions[add-zsh-hook]} )); then
    add-zsh-hook -d preexec _atuin_preexec 2>/dev/null
    add-zsh-hook -d precmd _atuin_precmd 2>/dev/null
    add-zsh-hook -d zshaddhistory _atuin_zshaddhistory 2>/dev/null
fi
unset ATUIN_HISTORY_ID HISTFILE
typeset -g _ZSH_AUTOSUGGEST_DISABLED=1
ZSH_AUTOSUGGEST_STRATEGY=()

zmodload zsh/zutil

# Divert candidates into __hits/__dscr instead of the completion set, so
# compsys never displays or inserts anything; print them for the reader.
# Adapted from Valodim/zsh-capture-completion (MIT).
compadd() {
    # Bookkeeping calls (-O/-A/-D) come from completion internals: pass through.
    if [[ ${@[1,(i)(-|--)]} == *-(O|A|D)\ * ]]; then
        builtin compadd "$@"
        return $?
    fi

    typeset -a __hits __dscr __tmp
    if (( $@[(I)-d] )); then
        __tmp=${@[$[${@[(i)-d]}+1]]}
        if [[ $__tmp == \(* ]]; then
            eval "__dscr=$__tmp"
        else
            __dscr=( "${(@P)__tmp}" )
        fi
    fi

    builtin compadd -A __hits -D __dscr "$@"

    setopt localoptions norcexpandparam extendedglob
    typeset -A apre hpre hsuf asuf
    zparseopts -E P:=apre p:=hpre S:=asuf s:=hsuf

    # Half-emulate -f: append / to directories.
    integer dirsuf=0
    if [[ -z $hsuf && "${${@//-default-/}% -# *}" == *-[[:alnum:]]#f* ]]; then
        dirsuf=1
    fi

    [[ -n $__hits ]] || return

    # Printing is a shell-level loop: cap it so huge candidate sets (every
    # command, giant directories) can't stall the oracle past its deadline.
    integer __max=$(( $#__hits > 100 ? 100 : $#__hits ))

    local dsuf dscr
    for i in {1..$__max}; do
        (( dirsuf )) && [[ -d $__hits[$i] ]] && dsuf=/ || dsuf=
        (( $#__dscr >= $i )) && dscr=$'\t'"${__dscr[$i]}" || dscr=
        print -r -- "$IPREFIX$apre$hpre$__hits[$i]$dsuf$hsuf$asuf$dscr"
    done
}

print -r -- __ATUIN_ORACLE_READY__
"#;

/// Drives bash's programmable completion headlessly: set the `COMP_*`
/// variables, call the registered `-F` function, print `COMPREPLY` between
/// NUL delimiters. Also strips anything the user's rc armed that could
/// invoke atuin per protocol line (bash-preexec's DEBUG trap feeds every
/// executed line to history hooks).
const BASH_INIT_SCRIPT: &str = r#"
if ! type -t _completion_loader >/dev/null 2>&1; then
    for __atuin_f in /usr/share/bash-completion/bash_completion /etc/bash_completion; do
        [[ -r $__atuin_f ]] && source "$__atuin_f" && break
    done
    unset __atuin_f
fi

trap - DEBUG
unset PROMPT_COMMAND HISTFILE
precmd_functions=() preexec_functions=()

# Only valid while readline drives a real completion; completers call it.
compopt() { :; }

__atuin_complete() {
    local line=$1
    COMP_LINE=$line
    COMP_POINT=${#line}
    local -a words=()
    read -r -a words <<< "$line"
    [[ -z $line || $line == *' ' ]] && words+=('')
    COMP_WORDS=("${words[@]}")
    COMP_CWORD=$(( ${#words[@]} - 1 ))
    local cmd=${COMP_WORDS[0]} cur=${COMP_WORDS[COMP_CWORD]}
    local prev=
    (( COMP_CWORD > 0 )) && prev=${COMP_WORDS[COMP_CWORD - 1]}
    COMPREPLY=()
    printf '\0\n'
    local spec fn
    spec=$(complete -p -- "$cmd" 2>/dev/null)
    if [[ -z $spec ]] && type -t _completion_loader >/dev/null 2>&1; then
        _completion_loader "$cmd" 2>/dev/null
        spec=$(complete -p -- "$cmd" 2>/dev/null)
    fi
    if [[ $spec == *' -F '* ]]; then
        fn=${spec#* -F }
        fn=${fn%% *}
        "$fn" "$cmd" "$cur" "$prev" 2>/dev/null
    elif [[ -n $spec ]]; then
        # Non-function specs (-W wordlists, -C commands, -G globs, ...):
        # compgen accepts the same generator options, and `complete -p`
        # output is canonically quoted for reuse as input.
        local args=${spec#complete }
        args=${args% "$cmd"}
        eval "mapfile -t COMPREPLY < <(compgen $args -- \"\$cur\" 2>/dev/null)"
    fi
    if (( ${#COMPREPLY[@]} == 0 )); then
        if (( COMP_CWORD == 0 )); then
            mapfile -t COMPREPLY < <(compgen -c -- "$cur" 2>/dev/null)
        else
            mapfile -t COMPREPLY < <(compgen -f -- "$cur" 2>/dev/null)
        fi
    fi
    local i
    for (( i = 0; i < ${#COMPREPLY[@]} && i < 100; i++ )); do
        printf '%s\n' "${COMPREPLY[i]}"
    done
    printf '\0\n'
}

echo __ATUIN_ORACLE_READY__
"#;

/// One shell completion: the token that replaces the current word, plus the
/// engine's description when it has one. The `token\tdescription` wire
/// format stays internal to each engine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub completion: String,
    pub description: Option<String>,
}

/// Which shell engine an oracle runs. zsh and bash are persistent captive
/// processes; fish's engine is headless by design, so it runs per query.
#[derive(Clone, Copy, Debug)]
pub enum OracleShell {
    Zsh,
    Bash,
    Fish,
}

/// Locate a binary on `PATH`.
pub fn find_in_path(name: &str) -> Option<std::path::PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

enum OracleProc {
    Zsh(ZshOracle),
    Bash(BashOracle),
    Fish(std::path::PathBuf),
}

impl OracleProc {
    fn spawn(shell: OracleShell, bin: &Path, load_user_config: bool) -> Option<Self> {
        match shell {
            OracleShell::Zsh => ZshOracle::spawn(bin, load_user_config).map(Self::Zsh),
            OracleShell::Bash => BashOracle::spawn(bin, load_user_config).map(Self::Bash),
            OracleShell::Fish => Some(Self::Fish(bin.to_path_buf())),
        }
    }

    fn complete(&mut self, line: &str, timeout: Duration) -> Option<Vec<Candidate>> {
        match self {
            Self::Zsh(oracle) => oracle.complete(line, timeout),
            Self::Bash(oracle) => oracle.complete(line, timeout),
            Self::Fish(fish) => Some(fish_complete(fish, line)),
        }
    }
}

/// fish's engine runs headless by design (`complete -C`), one process per
/// query — the only engine paying spawn cost per keystroke, but there is no
/// captive mode to keep warm. `--do-complete=$argv[1]` keeps the user's
/// line out of fish's parser: it arrives as an argument, never as code.
fn fish_complete(fish: &Path, line: &str) -> Vec<Candidate> {
    let output = std::process::Command::new(fish)
        .args(["--no-config", "-c", "complete --do-complete=$argv[1]"])
        .arg(line)
        .stdin(std::process::Stdio::null())
        .output();
    let Ok(output) = output else {
        return Vec::new();
    };
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(parse_candidate)
        .collect()
}

/// Async front door to the oracle: queries and answers cross a dedicated
/// thread, so a caller on a latency budget can stop waiting without
/// abandoning the wire protocol mid-read (which would desync it). Stale
/// answers are discarded by query id; a query arriving while the oracle is
/// busy simply replaces any queued one (latest wins).
pub struct CompletionOracleHandle {
    query_tx: mpsc::SyncSender<(u64, String)>,
    answer_rx: Receiver<(u64, Vec<Candidate>)>,
    next_id: u64,
}

impl CompletionOracleHandle {
    /// Start the oracle thread; the captive shell is respawned (up to a
    /// cap) if it wedges.
    pub fn spawn(shell: OracleShell, bin: std::path::PathBuf, load_user_config: bool) -> Self {
        let (query_tx, query_rx) = mpsc::sync_channel::<(u64, String)>(1);
        let (answer_tx, answer_rx) = mpsc::channel();

        std::thread::spawn(move || {
            let mut load_user_config = load_user_config;
            let mut spawns = 0u32;
            let respawn = |load_user_config: &mut bool, spawns: &mut u32| {
                if *spawns >= RESPAWN_LIMIT {
                    return None;
                }
                *spawns += 1;
                let proc = OracleProc::spawn(shell, &bin, *load_user_config);
                if proc.is_some() || !*load_user_config {
                    return proc;
                }
                // Their rc never produced a ready shell; retry hermetic
                // without burning another attempt.
                *load_user_config = false;
                OracleProc::spawn(shell, &bin, false)
            };

            // Warm up before the first keystroke needs an answer: spawning
            // (rc files, compinit) costs more than any query's wait budget.
            let mut proc = respawn(&mut load_user_config, &mut spawns);

            while let Ok((mut id, mut line)) = query_rx.recv() {
                // Only the newest queued query is worth answering.
                while let Ok((newer_id, newer_line)) = query_rx.try_recv() {
                    id = newer_id;
                    line = newer_line;
                }

                if proc.is_none() {
                    proc = respawn(&mut load_user_config, &mut spawns);
                }
                let Some(oracle) = proc.as_mut() else {
                    let _ = answer_tx.send((id, Vec::new()));
                    continue;
                };

                match oracle.complete(&line, QUERY_DEADLINE) {
                    Some(candidates) => {
                        if answer_tx.send((id, candidates)).is_err() {
                            return;
                        }
                    }
                    // Truly wedged: kill it; the next query respawns.
                    None => {
                        proc = None;
                        let _ = answer_tx.send((id, Vec::new()));
                    }
                }
            }
        });

        Self {
            query_tx,
            answer_rx,
            next_id: 0,
        }
    }

    /// Submit a query without waiting, so the caller can overlap other work
    /// (e.g. the history lookup) before collecting. `None` means the oracle
    /// is busy with one query already queued: this one loses.
    pub fn enqueue(&mut self, line: &str) -> Option<u64> {
        while self.answer_rx.try_recv().is_ok() {}

        self.next_id += 1;
        let id = self.next_id;
        self.query_tx.try_send((id, line.to_string())).ok()?;
        Some(id)
    }

    /// Wait up to `wait` for the answer to an enqueued query. A miss
    /// returns empty — the answer keeps computing and is discarded as stale
    /// later, and the oracle stays healthy for the next keystroke.
    pub fn collect(&mut self, id: u64, wait: Duration) -> Vec<Candidate> {
        let deadline = Instant::now() + wait;
        loop {
            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                return Vec::new();
            };
            match self.answer_rx.recv_timeout(remaining) {
                Ok((answer_id, candidates)) if answer_id == id => return candidates,
                Ok(_) => {} // stale answer to an abandoned query
                Err(_) => return Vec::new(),
            }
        }
    }

    /// [`Self::enqueue`] + [`Self::collect`] in one step.
    pub fn complete(&mut self, line: &str, wait: Duration) -> Vec<Candidate> {
        match self.enqueue(line) {
            Some(id) => self.collect(id, wait),
            None => Vec::new(),
        }
    }
}

/// Environment for a captive oracle shell. The `ATUIN_PTY_PROXY_*` guards
/// stop an rc that evals `atuin init` from exec-ing a proxy inside the
/// proxy; `ATUIN_SUGGEST_ORACLE` is an escape hatch so rc files can skip
/// heavy setup.
fn guard_env() -> [(&'static str, std::ffi::OsString); 3] {
    [
        ("ATUIN_PTY_PROXY_ACTIVE", "1".into()),
        (
            "ATUIN_PTY_PROXY_TMUX",
            std::env::var_os("TMUX").unwrap_or_default(),
        ),
        ("ATUIN_SUGGEST_ORACLE", "1".into()),
    ]
}

/// Write the init script to a temp file, source it in the captive shell,
/// and wait for its ready marker.
fn source_init(
    writer: &mut impl Write,
    lines: &Receiver<String>,
    script: &str,
    extension: &str,
    load_user_config: bool,
) -> bool {
    let init_path =
        std::env::temp_dir().join(format!("atuin-oracle-{}.{extension}", std::process::id()));
    if std::fs::write(&init_path, script).is_err() {
        return false;
    }
    let sourced = writeln!(writer, "source {}", init_path.display())
        .and_then(|()| writer.flush())
        .is_ok();
    let ready = sourced && await_ready(lines, spawn_deadline(load_user_config));
    let _ = std::fs::remove_file(&init_path);
    ready
}

pub(crate) struct ZshOracle {
    writer: Box<dyn Write + Send>,
    lines: Receiver<String>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
}

impl ZshOracle {
    /// Spawn a captive zsh and wait for its completion system to come up.
    ///
    /// With `load_user_config` the user's rc files run first, so their
    /// custom completions and fpath additions answer too; the caller falls
    /// back to a hermetic spawn if that shell never becomes ready.
    pub(crate) fn spawn(zsh: &Path, load_user_config: bool) -> Option<Self> {
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: 50,
                cols: 200,
                pixel_width: 0,
                pixel_height: 0,
            })
            .ok()?;

        // vt100 keeps ZLE alive (it refuses to run under TERM=dumb) with
        // minimal escape noise.
        let mut cmd = CommandBuilder::new(zsh);
        if load_user_config {
            cmd.args(["-i"]);
        } else {
            cmd.args(["-f", "-i"]);
        }
        cmd.env("TERM", "vt100");
        for (key, value) in guard_env() {
            cmd.env(key, value);
        }
        if let Ok(cwd) = std::env::current_dir() {
            cmd.cwd(cwd);
        }
        let child = pair.slave.spawn_command(cmd).ok()?;
        drop(pair.slave);

        let reader = pair.master.try_clone_reader().ok()?;
        let mut writer = pair.master.take_writer().ok()?;
        let lines = spawn_line_reader(reader);

        source_init(&mut writer, &lines, INIT_SCRIPT, "zsh", load_user_config).then_some(Self {
            writer,
            lines,
            child,
        })
    }

    /// Complete `line`. `None` means the oracle is desynced or dead: drop
    /// and respawn.
    pub(crate) fn complete(&mut self, line: &str, timeout: Duration) -> Option<Vec<Candidate>> {
        // Stale output would misattribute results to this query.
        while self.lines.try_recv().is_ok() {}

        self.writer.write_all(KILL_WHOLE_LINE).ok()?;
        self.writer.write_all(line.as_bytes()).ok()?;
        self.writer.write_all(b"\t").ok()?;
        self.writer.flush().ok()?;

        collect_candidates(&self.lines, Instant::now() + timeout)
    }
}

impl Drop for ZshOracle {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

/// Captive interactive bash over plain pipes: programmable completion needs
/// no terminal, so the driver function fakes the `COMP_*` environment and
/// prints `COMPREPLY` between NUL delimiters. Unlike the zsh oracle this
/// shell *executes* the protocol lines we send, so queries are passed as a
/// single-quoted argument.
pub(crate) struct BashOracle {
    stdin: std::process::ChildStdin,
    lines: Receiver<String>,
    child: std::process::Child,
}

impl BashOracle {
    /// Spawn a captive bash and wait for the completion driver to come up.
    ///
    /// With `load_user_config` bash runs interactively so `~/.bashrc` (and
    /// its custom completions) load; hermetic mode uses `--norc` and the
    /// system bash-completion only.
    pub(crate) fn spawn(bash: &Path, load_user_config: bool) -> Option<Self> {
        let mut cmd = std::process::Command::new(bash);
        // -i so rc files load and completion state behaves interactively;
        // with piped stdio readline stays out of the way (prompts land on
        // stderr, stdout carries only our protocol).
        if load_user_config {
            cmd.arg("-i");
        } else {
            cmd.args(["--norc", "-i"]);
        }
        cmd.env("TERM", "dumb");
        for (key, value) in guard_env() {
            cmd.env(key, value);
        }
        cmd.stdin(std::process::Stdio::piped());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());
        // Detach from the proxy's session: an interactive bash sharing our
        // controlling terminal fights us for it during job-control init.
        {
            use std::os::unix::process::CommandExt;
            unsafe {
                cmd.pre_exec(|| {
                    let _ = rustix::process::setsid();
                    Ok(())
                });
            }
        }

        let mut child = cmd.spawn().ok()?;
        let mut stdin = child.stdin.take()?;
        let stdout = child.stdout.take()?;
        let stderr = child.stderr.take()?;
        let lines = spawn_line_reader(stdout);
        // Prompts and rc noise arrive on stderr; drain it or a full pipe
        // buffer wedges the shell.
        std::thread::spawn(move || {
            let mut sink = stderr;
            let _ = std::io::copy(&mut sink, &mut std::io::sink());
        });

        source_init(
            &mut stdin,
            &lines,
            BASH_INIT_SCRIPT,
            "bash",
            load_user_config,
        )
        .then_some(Self {
            stdin,
            lines,
            child,
        })
    }

    /// Complete `line`. `None` means the oracle is desynced or dead: drop
    /// and respawn.
    pub(crate) fn complete(&mut self, line: &str, timeout: Duration) -> Option<Vec<Candidate>> {
        while self.lines.try_recv().is_ok() {}

        writeln!(self.stdin, "__atuin_complete {}", single_quoted(line)).ok()?;
        self.stdin.flush().ok()?;

        collect_candidates(&self.lines, Instant::now() + timeout)
    }
}

impl Drop for BashOracle {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Quote for a bash single-quoted context; the only special byte is `'`.
fn single_quoted(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

fn spawn_deadline(load_user_config: bool) -> Instant {
    let timeout = if load_user_config {
        SPAWN_TIMEOUT_USER_CONFIG
    } else {
        SPAWN_TIMEOUT_HERMETIC
    };
    Instant::now() + timeout
}

/// Split a byte stream into `\r?\n`-terminated lines on a thread, so
/// protocol reads can carry deadlines via `recv_timeout`.
fn spawn_line_reader(reader: impl Read + Send + 'static) -> Receiver<String> {
    let (line_tx, lines) = mpsc::channel();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(reader);
        let mut line = Vec::new();
        loop {
            line.clear();
            match reader.read_until(b'\n', &mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    while line.last().is_some_and(|&b| b == b'\n' || b == b'\r') {
                        line.pop();
                    }
                    if line_tx
                        .send(String::from_utf8_lossy(&line).into_owned())
                        .is_err()
                    {
                        return;
                    }
                }
            }
        }
    });
    lines
}

fn recv_line(lines: &Receiver<String>, deadline: Instant) -> Option<String> {
    let remaining = deadline.checked_duration_since(Instant::now())?;
    lines.recv_timeout(remaining).ok()
}

/// Skip startup noise until the init script's ready marker appears.
fn await_ready(lines: &Receiver<String>, deadline: Instant) -> bool {
    while let Some(line) = recv_line(lines, deadline) {
        if line.contains(READY_MARKER) && !line.contains("source ") {
            return true;
        }
    }
    false
}

/// Collect candidate lines between the two NUL-delimiter lines, dropping
/// display noise.
fn collect_candidates(lines: &Receiver<String>, deadline: Instant) -> Option<Vec<Candidate>> {
    let mut candidates = Vec::new();
    let mut in_results = false;
    loop {
        let received = recv_line(lines, deadline)?;
        if received.contains('\0') {
            if in_results {
                return Some(candidates);
            }
            in_results = true;
        } else if in_results && !received.contains('\x1b') {
            candidates.extend(parse_candidate(&received));
        }
    }
}

/// Parse one `token\tdescription` oracle line. Trailing spaces are trimmed:
/// bash completers append them as insert-a-space hints, which would leak
/// into the spliced suggestion.
fn parse_candidate(line: &str) -> Option<Candidate> {
    let (completion, description) = match line.split_once('\t') {
        Some((completion, description)) => (completion, Some(description)),
        None => (line, None),
    };
    let completion = completion.trim_end_matches(' ');
    if completion.is_empty() {
        return None;
    }
    Some(Candidate {
        completion: completion.to_string(),
        description: description
            .map(str::trim)
            .filter(|d| !d.is_empty())
            .map(str::to_string),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn completions(candidates: &[Candidate]) -> Vec<&str> {
        candidates.iter().map(|c| c.completion.as_str()).collect()
    }

    #[test]
    fn completes_against_real_bash() {
        let Some(bash) = find_in_path("bash") else {
            eprintln!("bash not installed; skipping oracle test");
            return;
        };
        if !std::path::Path::new("/usr/share/bash-completion/bash_completion").exists() {
            eprintln!("bash-completion not installed; skipping oracle test");
            return;
        }
        let mut oracle = BashOracle::spawn(&bash, false).expect("oracle spawns");

        let candidates = oracle
            .complete("git ch", Duration::from_secs(3))
            .expect("oracle answers");
        assert!(
            completions(&candidates).contains(&"checkout"),
            "git subcommands complete: {candidates:?}"
        );

        // Persistence and quoting: a second query containing a single quote
        // must not desync the protocol.
        let candidates = oracle
            .complete("echo 'a b' /tm", Duration::from_secs(3))
            .expect("oracle answers again");
        assert!(
            completions(&candidates).iter().any(|c| c.contains("tmp")),
            "file completion: {candidates:?}"
        );
        assert!(!completions(&candidates).contains(&"checkout"));
    }

    #[test]
    fn parses_candidate_lines() {
        assert_eq!(
            parse_candidate("checkout\tCheckout a branch"),
            Some(Candidate {
                completion: "checkout".to_string(),
                description: Some("Checkout a branch".to_string()),
            })
        );
        assert_eq!(
            parse_candidate("checkout "),
            Some(Candidate {
                completion: "checkout".to_string(),
                description: None,
            })
        );
        assert_eq!(parse_candidate(""), None);
        assert_eq!(parse_candidate("  "), None);
    }

    #[test]
    fn single_quoting_escapes_quotes() {
        assert_eq!(single_quoted("git ch"), "'git ch'");
        assert_eq!(single_quoted("echo 'hi'"), r"'echo '\''hi'\'''");
    }

    #[test]
    fn completes_against_real_zsh() {
        let Some(zsh) = find_in_path("zsh") else {
            eprintln!("zsh not installed; skipping oracle test");
            return;
        };
        let mut oracle = ZshOracle::spawn(&zsh, false).expect("oracle spawns");

        let candidates = oracle
            .complete("git ch", Duration::from_secs(3))
            .expect("oracle answers");
        assert!(
            completions(&candidates).contains(&"checkout"),
            "git subcommands complete: {candidates:?}"
        );

        // The oracle is persistent: a second, unrelated query must work and
        // not leak results from the first.
        let candidates = oracle
            .complete("cd /tm", Duration::from_secs(3))
            .expect("oracle answers again");
        assert!(
            completions(&candidates).iter().any(|c| c.contains("tmp")),
            "directory completion: {candidates:?}"
        );
        assert!(
            !completions(&candidates)
                .iter()
                .any(|c| c.contains("checkout"))
        );
    }
}
