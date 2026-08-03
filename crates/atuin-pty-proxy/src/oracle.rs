//! Headless zsh completion oracle.
//!
//! compsys only runs inside an interactive ZLE session, so a captive
//! `zsh -f -i` lives under its own pty. An init script overrides `compadd`
//! to divert every candidate into an array (so nothing is ever displayed or
//! inserted) and prints them between NUL-delimiter lines. One oracle
//! persists per session: compinit runs once, then each query is a
//! kill-line + text + Tab round trip.
//!
//! The `compadd` interception technique is adapted from
//! <https://github.com/Valodim/zsh-capture-completion> (MIT).

use std::io::{Read, Write};
use std::path::Path;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use portable_pty::{CommandBuilder, PtySize, native_pty_system};

const READY_MARKER: &str = "__ATUIN_ORACLE_READY__";
const SPAWN_TIMEOUT: Duration = Duration::from_secs(5);
const KILL_WHOLE_LINE: &[u8] = b"\x15";

/// Delimits completion output; re-arms itself because zsh clears the
/// pre/post function arrays after every completion.
const INIT_SCRIPT: &str = r#"
PROMPT=''
RPROMPT=''
unset zle_bracketed_paste 2>/dev/null

autoload -Uz compinit
compinit -C -d "${TMPDIR:-/tmp}/.atuin-oracle-zcompdump"

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

    local dsuf dscr
    for i in {1..$#__hits}; do
        (( dirsuf )) && [[ -d $__hits[$i] ]] && dsuf=/ || dsuf=
        (( $#__dscr >= $i )) && dscr=$'\t'"${__dscr[$i]}" || dscr=
        print -r -- "$IPREFIX$apre$hpre$__hits[$i]$dsuf$hsuf$asuf$dscr"
    done
}

print -r -- __ATUIN_ORACLE_READY__
"#;

pub struct ZshOracle {
    writer: Box<dyn Write + Send>,
    lines: Receiver<String>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
}

impl ZshOracle {
    /// Spawn a captive zsh and wait for its completion system to come up.
    pub fn spawn(zsh: &Path) -> Option<Self> {
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: 50,
                cols: 200,
                pixel_width: 0,
                pixel_height: 0,
            })
            .ok()?;

        // -f: no user rc files — hermetic and fast. vt100 keeps ZLE alive
        // (it refuses to run under TERM=dumb) with minimal escape noise.
        let mut cmd = CommandBuilder::new(zsh);
        cmd.args(["-f", "-i"]);
        cmd.env("TERM", "vt100");
        if let Ok(cwd) = std::env::current_dir() {
            cmd.cwd(cwd);
        }
        let child = pair.slave.spawn_command(cmd).ok()?;
        drop(pair.slave);

        let mut reader = pair.master.try_clone_reader().ok()?;
        let mut writer = pair.master.take_writer().ok()?;

        let (line_tx, lines) = mpsc::channel();
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            let mut pending = Vec::new();
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        pending.extend_from_slice(&buf[..n]);
                        while let Some(end) = pending.iter().position(|&b| b == b'\n') {
                            let mut line: Vec<u8> = pending.drain(..=end).collect();
                            line.pop();
                            if line.last() == Some(&b'\r') {
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
            }
        });

        let init_path =
            std::env::temp_dir().join(format!("atuin-oracle-{}.zsh", std::process::id()));
        std::fs::write(&init_path, INIT_SCRIPT).ok()?;
        writeln!(writer, "source {}", init_path.display()).ok()?;
        writer.flush().ok()?;

        let mut oracle = Self {
            writer,
            lines,
            child,
        };

        let deadline = Instant::now() + SPAWN_TIMEOUT;
        let ready = loop {
            match oracle.recv_line(deadline) {
                Some(line) if line.contains(READY_MARKER) && !line.contains("source ") => {
                    break true;
                }
                Some(_) => {}
                None => break false,
            }
        };
        let _ = std::fs::remove_file(&init_path);
        ready.then_some(oracle)
    }

    /// Complete `line`, returning raw `candidate\tdescription` lines.
    /// `None` means the oracle is desynced or dead: drop and respawn.
    pub fn complete(&mut self, line: &str, timeout: Duration) -> Option<Vec<String>> {
        // Stale output would misattribute results to this query.
        while self.lines.try_recv().is_ok() {}

        self.writer.write_all(KILL_WHOLE_LINE).ok()?;
        self.writer.write_all(line.as_bytes()).ok()?;
        self.writer.write_all(b"\t").ok()?;
        self.writer.flush().ok()?;

        let deadline = Instant::now() + timeout;
        let mut candidates = Vec::new();
        let mut in_results = false;
        loop {
            let received = self.recv_line(deadline)?;
            if received.contains('\0') {
                if in_results {
                    return Some(candidates);
                }
                in_results = true;
            } else if in_results && !received.is_empty() && !received.contains('\x1b') {
                candidates.push(received);
            }
        }
    }

    fn recv_line(&mut self, deadline: Instant) -> Option<String> {
        let remaining = deadline.checked_duration_since(Instant::now())?;
        self.lines.recv_timeout(remaining).ok()
    }
}

impl Drop for ZshOracle {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zsh_path() -> Option<std::path::PathBuf> {
        std::env::var_os("PATH").and_then(|path| {
            std::env::split_paths(&path)
                .map(|dir| dir.join("zsh"))
                .find(|candidate| candidate.is_file())
        })
    }

    #[test]
    fn completes_against_real_zsh() {
        let Some(zsh) = zsh_path() else {
            eprintln!("zsh not installed; skipping oracle test");
            return;
        };
        let mut oracle = ZshOracle::spawn(&zsh).expect("oracle spawns");

        let candidates = oracle
            .complete("git ch", Duration::from_secs(3))
            .expect("oracle answers");
        let tokens: Vec<&str> = candidates
            .iter()
            .map(|c| c.split('\t').next().unwrap_or_default())
            .collect();
        assert!(
            tokens.contains(&"checkout"),
            "git subcommands complete: {tokens:?}"
        );

        // The oracle is persistent: a second, unrelated query must work and
        // not leak results from the first.
        let candidates = oracle
            .complete("cd /tm", Duration::from_secs(3))
            .expect("oracle answers again");
        assert!(
            candidates.iter().any(|c| c.contains("tmp")),
            "directory completion: {candidates:?}"
        );
        assert!(!candidates.iter().any(|c| c.contains("checkout")));
    }
}
