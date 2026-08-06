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
/// How long the oracle stays unspawned after proxy start unless a warm
/// nudge (the session's first prompt) or a query arrives first: its
/// rc-loading captive shell must not compete with the session shell's
/// own startup.
const WARM_SPAWN_DELAY: Duration = Duration::from_secs(5);
const KILL_WHOLE_LINE: &[u8] = b"\x15";

/// Turns the captive zsh into a completion driver: Tab is the only live
/// binding, atuin's own hooks and autosuggestions are stripped, candidates
/// are diverted through a `compadd` override, and pre/post completion hooks
/// bracket the output with NUL delimiter lines for the reader.
const INIT_SCRIPT: &str = r#"
PROMPT=''
RPROMPT=''
unset zle_bracketed_paste 2>/dev/null

# The user's rc may already have run compinit; don't clobber its setup.
# The dump lives in the oracle's own 0700 directory: `-C` skips zsh's
# insecure-file check, so a dump anyone else could write is code execution.
if ! (( ${+functions[compdef]} )); then
    autoload -Uz compinit
    compinit -C -d @ATUIN_ZCOMPDUMP@
fi

# The oracle must never run a command, only complete.
bindkey '^M' undefined
bindkey '^J' undefined
bindkey '^I' complete-word

# zsh clears these arrays after every completion, so each hook re-arms itself.
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

/// Lifecycle breadcrumbs behind `ATUIN_PTY_PROXY_TRACE=1`, matching the
/// runtime's startup trace: when tabs feel slow, the oracle's spawn
/// timing is the first thing to rule in or out.
fn trace(message: &str) {
    if crate::pty_proxy::env_flag("ATUIN_PTY_PROXY_TRACE") {
        eprintln!("atuin pty-proxy: trace: {message}\r");
    }
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
    Fish {
        bin: std::path::PathBuf,
        load_user_config: bool,
    },
}

impl OracleProc {
    fn spawn(shell: OracleShell, bin: &Path, load_user_config: bool) -> Option<Self> {
        match shell {
            OracleShell::Zsh => ZshOracle::spawn(bin, load_user_config).map(Self::Zsh),
            OracleShell::Bash => BashOracle::spawn(bin, load_user_config).map(Self::Bash),
            OracleShell::Fish => Some(Self::Fish {
                bin: bin.to_path_buf(),
                load_user_config,
            }),
        }
    }

    fn complete(&mut self, line: &str, timeout: Duration) -> Option<Vec<Candidate>> {
        match self {
            Self::Zsh(oracle) => oracle.complete(line, timeout),
            Self::Bash(oracle) => oracle.complete(line, timeout),
            Self::Fish {
                bin,
                load_user_config,
            } => fish_complete(bin, line, *load_user_config, timeout),
        }
    }

    /// Whether a missed deadline means this engine is unusable. zsh and bash
    /// hold a persistent shell whose NUL framing desyncs when a query is
    /// abandoned mid-protocol, so it must be replaced. fish runs a fresh
    /// process per query and holds no state to corrupt: a slow spawn there
    /// is transient, and must not spend a respawn from the session's budget.
    fn is_captive(&self) -> bool {
        matches!(self, Self::Zsh(_) | Self::Bash(_))
    }
}

/// fish's engine runs headless by design (`complete -C`), one process per
/// query — the only engine paying spawn cost per keystroke, but there is no
/// captive mode to keep warm. `--do-complete=$argv[1]` keeps the user's
/// line out of fish's parser: it arrives as an argument, never as code. User
/// configuration loads only when fish matches the session shell.
fn fish_complete(
    fish: &Path,
    line: &str,
    load_user_config: bool,
    timeout: Duration,
) -> Option<Vec<Candidate>> {
    let mut command = fish_command(fish, line, load_user_config);
    let output = command_stdout_with_timeout(&mut command, timeout)?;

    Some(
        String::from_utf8_lossy(&output)
            .lines()
            .filter_map(parse_candidate)
            .collect(),
    )
}

fn fish_command(fish: &Path, line: &str, load_user_config: bool) -> std::process::Command {
    use std::os::unix::process::CommandExt;

    let mut command = std::process::Command::new(fish);
    if !load_user_config {
        command.arg("--no-config");
    }
    command
        .args(["-c", "complete --do-complete=$argv[1]"])
        .arg(line)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    unsafe {
        command.pre_exec(|| {
            rustix::process::setsid()?;
            Ok(())
        });
    }

    command
}

fn command_stdout_with_timeout(
    command: &mut std::process::Command,
    timeout: Duration,
) -> Option<Vec<u8>> {
    command.stdout(std::process::Stdio::piped());
    let mut child = command.spawn().ok()?;
    let Some(mut stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return None;
    };
    let reader = std::thread::spawn(move || {
        let mut output = Vec::new();
        stdout.read_to_end(&mut output).map(|_| output)
    });
    let deadline = Instant::now() + timeout;
    let mut exited = false;

    loop {
        if !exited {
            match child.try_wait() {
                Ok(Some(_)) => exited = true,
                Ok(None) => {}
                Err(_) => {
                    kill_std_process_group(&mut child);
                    let _ = reader.join();
                    return None;
                }
            }
        }
        if exited && reader.is_finished() {
            return reader.join().ok()?.ok();
        }

        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            kill_std_process_group(&mut child);
            if !exited {
                let _ = child.wait();
            }
            let _ = reader.join();
            return None;
        };
        std::thread::sleep(remaining.min(Duration::from_millis(5)));
    }
}

fn kill_std_process_group(child: &mut std::process::Child) {
    let pid = rustix::process::Pid::from_raw(child.id() as i32);
    let killed = pid.is_some_and(|pid| {
        rustix::process::kill_process_group(pid, rustix::process::Signal::KILL).is_ok()
    });
    if !killed {
        match child.try_wait() {
            Ok(Some(_)) => {}
            Ok(None) | Err(_) => {
                let _ = child.kill();
            }
        }
    }
    let _ = child.wait();
}

fn kill_pty_process_group(child: &mut dyn portable_pty::Child) {
    let killed = child
        .process_id()
        .and_then(|pid| rustix::process::Pid::from_raw(pid as i32))
        .is_some_and(|pid| {
            rustix::process::kill_process_group(pid, rustix::process::Signal::KILL).is_ok()
        });
    if !killed {
        let _ = child.kill();
    }
    let _ = child.wait();
}

/// Async front door to the oracle: queries and answers cross a dedicated
/// thread, so a caller on a latency budget can stop waiting without
/// abandoning the wire protocol mid-read (which would desync it). Stale
/// answers are discarded by query id; the worker skips to the newest
/// queued query, and [`Self::enqueue`] rejects a query outright while an
/// unread one already sits in the (single-slot) queue.
pub struct CompletionOracleHandle {
    query_tx: mpsc::SyncSender<OracleRequest>,
    answer_rx: Receiver<OracleAnswer>,
    next_id: u64,
}

/// Nudges the oracle to spawn its captive shell now — fired when the
/// session's first prompt appears, so the rc-loading happens while the
/// user reads the prompt instead of during shell startup or on the first
/// keystroke. Cheap to clone; safe from any thread.
#[derive(Clone)]
pub struct OracleWarmer(mpsc::SyncSender<OracleRequest>);

impl OracleWarmer {
    pub fn warm(&self) {
        let _ = self.0.try_send(OracleRequest::Warm);
    }
}

enum OracleRequest {
    /// Spawn the captive shell now; carries no query.
    Warm,
    Query(OracleQuery),
}

struct OracleQuery {
    id: u64,
    line: String,
}

struct OracleAnswer {
    id: u64,
    candidates: Vec<Candidate>,
}

impl CompletionOracleHandle {
    /// Start the oracle thread; the captive shell is respawned (up to a
    /// cap) if it wedges.
    pub fn spawn(shell: OracleShell, bin: std::path::PathBuf, load_user_config: bool) -> Self {
        let (query_tx, query_rx) = mpsc::sync_channel::<OracleRequest>(1);
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

            // The captive shell loads the user's rc files — exactly what
            // the tab's own shell is doing at this moment. Spawning both
            // at once makes them fight over CPU and compinit's zcompdump
            // lock, visibly delaying the prompt to serve a popup nobody
            // has asked for yet. Wait for the warm nudge (the session's
            // first prompt), the first query, or a quiet delay.
            let mut pending = match query_rx.recv_timeout(WARM_SPAWN_DELAY) {
                Ok(OracleRequest::Query(query)) => Some(query),
                Ok(OracleRequest::Warm) | Err(mpsc::RecvTimeoutError::Timeout) => None,
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            };
            let spawn_started = Instant::now();
            trace(&format!("oracle spawn begins ({shell:?})"));
            let mut proc = respawn(&mut load_user_config, &mut spawns);
            trace(&format!(
                "oracle {} after {:?}",
                if proc.is_some() { "ready" } else { "failed" },
                spawn_started.elapsed()
            ));

            loop {
                let mut query = match pending.take() {
                    Some(query) => query,
                    None => loop {
                        match query_rx.recv() {
                            Ok(OracleRequest::Query(query)) => break query,
                            // Already warm; nothing to do.
                            Ok(OracleRequest::Warm) => {}
                            Err(_) => return,
                        }
                    },
                };
                // Only the newest queued query is worth answering.
                while let Ok(newer) = query_rx.try_recv() {
                    if let OracleRequest::Query(newer) = newer {
                        query = newer;
                    }
                }

                if proc.is_none() {
                    proc = respawn(&mut load_user_config, &mut spawns);
                }
                let answer = |candidates| OracleAnswer {
                    id: query.id,
                    candidates,
                };
                let Some(oracle) = proc.as_mut() else {
                    let _ = answer_tx.send(answer(Vec::new()));
                    continue;
                };

                match oracle.complete(&query.line, QUERY_DEADLINE) {
                    Some(candidates) => {
                        if answer_tx.send(answer(candidates)).is_err() {
                            return;
                        }
                    }
                    // A captive shell that misses its deadline is wedged
                    // mid-protocol: drop it so the next query respawns. A
                    // per-query engine has nothing to wedge, so keep it —
                    // dropping it would burn respawns until the budget ran
                    // out and completions stopped for the session.
                    None => {
                        if oracle.is_captive() {
                            proc = None;
                        }
                        let _ = answer_tx.send(answer(Vec::new()));
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
    /// (e.g. the history lookup) before collecting. `None` means the query
    /// was not accepted: an unread one is already queued, or the oracle
    /// thread has exited.
    pub fn enqueue(&mut self, line: &str) -> Option<u64> {
        while self.answer_rx.try_recv().is_ok() {}

        self.next_id += 1;
        let id = self.next_id;
        self.query_tx
            .try_send(OracleRequest::Query(OracleQuery {
                id,
                line: line.to_string(),
            }))
            .ok()?;
        Some(id)
    }

    /// A cloneable early-spawn nudge; see [`OracleWarmer`].
    pub fn warmer(&self) -> OracleWarmer {
        OracleWarmer(self.query_tx.clone())
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
                Ok(answer) if answer.id == id => return answer.candidates,
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

/// A private directory for the files the captive shell reads: its init
/// script, and (for zsh) the completion dump `compinit` loads.
///
/// The shell *executes* both as the user, so neither may live at a
/// predictable path in a shared temp directory: another local user could
/// pre-create the file, or replace it between the write and the `source`,
/// and have their code run under this uid. `compinit -C` makes the dump the
/// sharper edge of the two — it skips zsh's own insecure-file check.
///
/// Mirrors the per-proxy directory in [`crate::screen`]: mode 0700, an
/// unpredictable name, and `create` (never `create_all`), so a name an
/// attacker got to first fails the spawn instead of being reused.
struct OracleDir(std::path::PathBuf);

impl OracleDir {
    fn new() -> Option<Self> {
        use rand::RngCore;
        use std::os::unix::fs::DirBuilderExt;

        let mut suffix = [0u8; 8];
        rand::thread_rng().fill_bytes(&mut suffix);
        let path = std::env::temp_dir().join(format!(
            "atuin-oracle-{}-{}",
            std::process::id(),
            crate::screen::hex_encode(&suffix)
        ));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&path)
            .ok()
            .map(|()| Self(path))
    }

    fn join(&self, name: &str) -> std::path::PathBuf {
        self.0.join(name)
    }
}

impl Drop for OracleDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// POSIX single-quoting — the one form a shell expands nothing inside — for
/// paths interpolated into a script line. `$TMPDIR` is the user's own, but a
/// path with a space or a `$` in it must still reach the shell intact.
fn shell_quote(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', r"'\''"))
}

/// Write the init script into the oracle's private directory, source it in
/// the captive shell, and wait for its ready marker.
fn source_init(
    writer: &mut impl Write,
    lines: &Receiver<String>,
    script: &str,
    dir: &OracleDir,
    extension: &str,
    load_user_config: bool,
) -> bool {
    use std::os::unix::fs::OpenOptionsExt;

    let init_path = dir.join(&format!("init.{extension}"));
    let written = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&init_path)
        .and_then(|mut file| file.write_all(script.as_bytes()));
    if written.is_err() {
        return false;
    }
    let sourced = writeln!(writer, "source {}", shell_quote(&init_path))
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
    /// Holds the completion dump for the shell's lifetime; dropping it
    /// removes the directory.
    _dir: OracleDir,
}

impl ZshOracle {
    /// Spawn a captive zsh and wait for its completion system to come up.
    ///
    /// With `load_user_config` the user's rc files run first, so their
    /// custom completions and fpath additions answer too; the caller falls
    /// back to a hermetic spawn if that shell never becomes ready.
    pub(crate) fn spawn(zsh: &Path, load_user_config: bool) -> Option<Self> {
        let dir = OracleDir::new()?;
        let script = INIT_SCRIPT.replace("@ATUIN_ZCOMPDUMP@", &shell_quote(&dir.join("zcompdump")));
        let pair = native_pty_system()
            // Tall enough that compsys never paginates its candidate
            // list, wide enough that long candidates don't wrap mid-token.
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
        let mut child = pair.slave.spawn_command(cmd).ok()?;
        drop(pair.slave);

        let reader = match pair.master.try_clone_reader() {
            Ok(reader) => reader,
            Err(_) => {
                kill_pty_process_group(child.as_mut());
                return None;
            }
        };
        let mut writer = match pair.master.take_writer() {
            Ok(writer) => writer,
            Err(_) => {
                kill_pty_process_group(child.as_mut());
                return None;
            }
        };
        let lines = spawn_line_reader(reader);

        if !source_init(&mut writer, &lines, &script, &dir, "zsh", load_user_config) {
            kill_pty_process_group(child.as_mut());
            return None;
        }

        Some(Self {
            writer,
            lines,
            child,
            _dir: dir,
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
        kill_pty_process_group(self.child.as_mut());
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
    /// Removes the private init-script directory when the shell goes away.
    _dir: OracleDir,
}

impl BashOracle {
    /// Spawn a captive bash and wait for the completion driver to come up.
    ///
    /// With `load_user_config` bash runs interactively so `~/.bashrc` (and
    /// its custom completions) load; hermetic mode uses `--norc` and the
    /// system bash-completion only.
    pub(crate) fn spawn(bash: &Path, load_user_config: bool) -> Option<Self> {
        let dir = OracleDir::new()?;
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
            &dir,
            "bash",
            load_user_config,
        )
        .then_some(Self {
            stdin,
            lines,
            child,
            _dir: dir,
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
        if line.contains(READY_MARKER) {
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
    fn fish_config_flag_matches_the_session() {
        let configured = fish_command(Path::new("/fish"), "git ch", true);
        assert!(!configured.get_args().any(|arg| arg == "--no-config"));

        let hermetic = fish_command(Path::new("/fish"), "git ch", false);
        assert!(hermetic.get_args().any(|arg| arg == "--no-config"));
    }

    #[test]
    fn command_output_honors_its_deadline() {
        let mut command = std::process::Command::new("/bin/sh");
        command.args(["-c", "sleep 2"]);
        unsafe {
            use std::os::unix::process::CommandExt;
            command.pre_exec(|| {
                rustix::process::setsid()?;
                Ok(())
            });
        }
        let started = Instant::now();

        assert!(command_stdout_with_timeout(&mut command, Duration::from_millis(50)).is_none());
        assert!(started.elapsed() < Duration::from_secs(1));
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
