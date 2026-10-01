//! Whether a harness process on this machine may be writing a session.
//!
//! Each harness says so differently, and some not at all:
//!
//! - **Claude Code** registers every running session (interactive, `--print` and background
//!   alike) in `<config dir>/sessions/<pid>.json`, rewriting its `sessionId` as the process moves
//!   between sessions, and removes it on exit. A crash leaves it behind, which the pid (and the
//!   start time Claude Code records with it) tells apart.
//! - **Codex** holds an exclusive `flock` on `$CODEX_HOME/thread-writer-locks/<thread>.lock` for
//!   as long as a process has the thread loaded, and removes the file after.
//! - **opencode** and **pi** record nothing, so their processes are looked for: one that names
//!   the session on its command line is live, one that names another is not, and one that runs
//!   in the session's directory without saying which session it has open may be writing it. pi
//!   replaces its command line with its name (`process.title = "pi"`), so a pi process never
//!   says which session it has open.

use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};

/// Whether a harness process on this machine may be writing a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Liveness {
    /// A harness process has the session open: process `pid`, when that is known.
    Live {
        pid: Option<u32>,
    },
    /// Nothing on this machine has the session open.
    NotLive,
    /// A harness process may have the session open, and which session it has open cannot be
    /// told.
    Unknown,
}

/// One running process, as much of it as can be read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProcessInfo {
    pub pid: u32,
    /// Its command line (as the process presents it now: a program may rewrite it).
    pub argv: Vec<String>,
    /// Its name (Linux `comm`).
    pub name: Option<String>,
    pub cwd: Option<PathBuf>,
}

/// Where the processes of this machine are read from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Processes {
    /// A `proc` filesystem mounted at this path (Linux: `/proc`; tests: a directory laid out
    /// like one).
    ProcFs(PathBuf),
    /// The platform's own process table, through `sysinfo` (macOS `proc_pidinfo`, which unlike
    /// `ps` also has each process's working directory).
    System,
}

/// A process that is running, and when it started as Claude Code records it (`procStart`: on
/// Linux, the `starttime` field of `/proc/<pid>/stat`), when that can be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Running {
    pub start: Option<String>,
}

impl Processes {
    /// This machine's processes.
    #[must_use]
    pub fn here() -> Self {
        if cfg!(target_os = "linux") {
            Self::ProcFs(PathBuf::from("/proc"))
        } else {
            Self::System
        }
    }

    /// Every process that can be seen.
    pub fn list(&self) -> io::Result<Vec<ProcessInfo>> {
        match self {
            Self::ProcFs(root) => {
                let mut out = Vec::new();
                for entry in std::fs::read_dir(root)? {
                    let Ok(entry) = entry else {
                        continue;
                    };
                    let Some(pid) = entry.file_name().to_str().and_then(|n| n.parse::<u32>().ok())
                    else {
                        continue;
                    };
                    let dir = entry.path();
                    // A process that exits while it is read is simply not there.
                    let Ok(cmdline) = std::fs::read(dir.join("cmdline")) else {
                        continue;
                    };
                    if proc_stat(&dir).is_some_and(|stat| stat.zombie) {
                        continue;
                    }
                    out.push(ProcessInfo {
                        pid,
                        argv: split_cmdline(&cmdline),
                        name: std::fs::read_to_string(dir.join("comm"))
                            .ok()
                            .map(|n| n.trim_end().to_owned()),
                        cwd: std::fs::read_link(dir.join("cwd")).ok(),
                    });
                }
                Ok(out)
            }
            Self::System => {
                use sysinfo::{ProcessRefreshKind, ProcessStatus, ProcessesToUpdate, UpdateKind};
                let mut system = sysinfo::System::new();
                system.refresh_processes_specifics(
                    ProcessesToUpdate::All,
                    true,
                    ProcessRefreshKind::nothing()
                        .with_cmd(UpdateKind::Always)
                        .with_cwd(UpdateKind::Always),
                );
                Ok(system
                    .processes()
                    .values()
                    .filter(|p| p.thread_kind().is_none() && p.status() != ProcessStatus::Zombie)
                    .map(|p| ProcessInfo {
                        pid: p.pid().as_u32(),
                        argv: p.cmd().iter().map(|a| a.to_string_lossy().into_owned()).collect(),
                        name: Some(p.name().to_string_lossy().into_owned()),
                        cwd: p.cwd().map(Path::to_path_buf),
                    })
                    .collect())
            }
        }
    }

    /// Process `pid`, if it is running.
    #[must_use]
    pub fn running(&self, pid: u32) -> Option<Running> {
        match self {
            Self::ProcFs(root) => {
                let stat = proc_stat(&root.join(pid.to_string()))?;
                (!stat.zombie).then_some(Running { start: stat.start })
            }
            Self::System => {
                let pid = sysinfo::Pid::from_u32(pid);
                let mut system = sysinfo::System::new();
                system.refresh_processes_specifics(
                    sysinfo::ProcessesToUpdate::Some(&[pid]),
                    true,
                    sysinfo::ProcessRefreshKind::nothing(),
                );
                // Claude Code records `ps -o lstart` there, which is not compared.
                system
                    .process(pid)
                    .filter(|p| p.status() != sysinfo::ProcessStatus::Zombie)
                    .map(|_| Running { start: None })
            }
        }
    }
}

struct Stat {
    zombie: bool,
    start: Option<String>,
}

/// `<dir>/stat`: whether the process is a zombie, and its `starttime` (field 22), read the way
/// Claude Code reads it: after the command name's closing parenthesis.
fn proc_stat(dir: &Path) -> Option<Stat> {
    let stat = std::fs::read_to_string(dir.join("stat")).ok()?;
    let rest = &stat[stat.rfind(')')? + 1..];
    let fields: Vec<&str> = rest.split_whitespace().collect();
    Some(Stat {
        zombie: fields.first() == Some(&"Z"),
        start: fields.get(19).map(|s| (*s).to_owned()),
    })
}

fn split_cmdline(raw: &[u8]) -> Vec<String> {
    // Each argument ends in a NUL; a program that rewrote its command line pads it with more.
    let mut args: Vec<String> =
        raw.split(|b| *b == 0).map(|arg| String::from_utf8_lossy(arg).into_owned()).collect();
    while args.last().is_some_and(String::is_empty) {
        args.pop();
    }
    args
}

/// The file name of `arg`, without an `.exe`.
fn program_name(arg: &str) -> &str {
    let name = arg.rsplit(['/', '\\']).next().unwrap_or(arg);
    name.strip_suffix(".exe").unwrap_or(name)
}

/// What a process says of the sessions it may write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Claim {
    /// Not a harness process that writes sessions.
    None,
    /// It has the session named on its command line open: this one or not.
    Session(bool),
    /// It works on a session of the directory it runs in (or of `dir`), which one unsaid.
    Directory(Option<PathBuf>),
    /// It may write any session (a server).
    Any,
}

/// The value of `--name` / `-short` in `args`, in either `--name value` or `--name=value` form.
fn flag_value<'a>(args: &'a [String], names: &[&str]) -> Option<&'a str> {
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        for name in names {
            if arg == name {
                return iter.next().map(String::as_str);
            }
            if let Some(value) = arg.strip_prefix(name).and_then(|rest| rest.strip_prefix('=')) {
                return Some(value);
            }
        }
    }
    None
}

/// The arguments of an opencode process, when `process` is one.
fn opencode_args(process: &ProcessInfo) -> Option<&[String]> {
    match process.argv.as_slice() {
        [program, rest @ ..] if program_name(program) == "opencode" => Some(rest),
        // A script run by its interpreter (a development checkout).
        [_, script, rest @ ..] if program_name(script) == "opencode" => Some(rest),
        [] if process.name.as_deref() == Some("opencode") => Some(&[]),
        _ => None,
    }
}

/// opencode's commands that never write a session (opencode 1.18 `cli/cmd`), or that leave it
/// to a server process of their own (`attach`, and `pr`, which starts the TUI as a child).
const OPENCODE_READERS: &[&str] = &[
    "mcp",
    "attach",
    "generate",
    "debug",
    "auth",
    "providers",
    "agent",
    "upgrade",
    "uninstall",
    "models",
    "stats",
    "export",
    "import",
    "session",
    "plugin",
    "plug",
    "db",
    "completion",
    "pr",
];

/// The commands whose process serves any session asked of it.
const OPENCODE_SERVERS: &[&str] = &["serve", "web", "acp", "github"];

/// What an opencode process claims of session `id`.
pub(crate) fn opencode_claim(process: &ProcessInfo, id: &str) -> Claim {
    let Some(args) = opencode_args(process) else {
        return Claim::None;
    };
    if args.iter().any(|a| matches!(a.as_str(), "-h" | "--help" | "-v" | "--version")) {
        return Claim::None;
    }
    // The first word that is no flag: a command, else the TUI's project directory. Flags that
    // take a value are few enough to skip by name.
    let mut positional = None;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if matches!(
            arg.as_str(),
            "-s" | "--session"
                | "-m"
                | "--model"
                | "--agent"
                | "--prompt"
                | "--port"
                | "--hostname"
                | "--log-level"
                | "--dir"
        ) {
            iter.next();
        } else if !arg.starts_with('-') {
            positional = Some(arg.as_str());
            break;
        }
    }
    match positional {
        Some(command) if OPENCODE_READERS.contains(&command) => return Claim::None,
        Some(command) if OPENCODE_SERVERS.contains(&command) => return Claim::Any,
        _ => {}
    }
    if let Some(session) = flag_value(args, &["--session", "-s"]) {
        return Claim::Session(session == id);
    }
    let dir = positional.filter(|p| *p != "run").map(PathBuf::from);
    Claim::Directory(dir.or_else(|| flag_value(args, &["--dir"]).map(PathBuf::from)))
}

/// What a pi process claims of session `id`, whose file is `path` when known.
pub(crate) fn pi_claim(process: &ProcessInfo, id: &str, path: Option<&Path>) -> Claim {
    let args = match process.argv.as_slice() {
        [program, rest @ ..] if program_name(program) == "pi" => rest,
        [_, script, rest @ ..]
            if program_name(script) == "pi" || script.contains("pi-coding-agent") =>
        {
            rest
        }
        _ if process.name.as_deref() == Some("pi") => &[],
        _ => return Claim::None,
    };
    if args.iter().any(|a| {
        matches!(a.as_str(), "-h" | "--help" | "-v" | "--version" | "--no-session" | "--export")
    }) {
        return Claim::None;
    }
    if let Some(first) = args.first()
        && matches!(
            first.as_str(),
            "install" | "remove" | "uninstall" | "update" | "list" | "config"
        )
    {
        return Claim::None;
    }
    // `--fork` reads the session it names and writes a new one.
    if flag_value(args, &["--fork"]).is_some() {
        return Claim::Session(false);
    }
    if let Some(session) = flag_value(args, &["--session"]) {
        let named = Path::new(session);
        let ours = session == id
            || (!session.is_empty() && id.starts_with(session))
            || path.is_some_and(|path| named == path || named.file_name() == path.file_name())
            || named.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.contains(id));
        return Claim::Session(ours);
    }
    Claim::Directory(None)
}

/// Whether `a` and `b` are the same directory.
fn same_dir(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// Whether any of `processes` may be writing a session that works in `cwd`, by what each
/// `claim`s: live if one names it; unknown if one serves any session, or works in its
/// directory (or in one that cannot be read) without naming a session.
pub(crate) fn scan(
    processes: &[ProcessInfo],
    cwd: Option<&Path>,
    claim: impl Fn(&ProcessInfo) -> Claim,
) -> Liveness {
    let mut unknown = false;
    for process in processes {
        match claim(process) {
            Claim::None | Claim::Session(false) => {}
            Claim::Session(true) => {
                return Liveness::Live {
                    pid: Some(process.pid),
                };
            }
            Claim::Any => unknown = true,
            Claim::Directory(dir) => {
                let dir = match (dir, &process.cwd) {
                    (Some(dir), Some(pwd)) if dir.is_relative() => Some(pwd.join(dir)),
                    (Some(dir), _) => Some(dir),
                    (None, pwd) => pwd.clone(),
                };
                unknown |= match (dir, cwd) {
                    (Some(dir), Some(cwd)) => same_dir(&dir, cwd),
                    // A process whose directory cannot be read, or a session whose directory
                    // is not known, cannot be told apart.
                    _ => true,
                };
            }
        }
    }
    if unknown {
        Liveness::Unknown
    } else {
        Liveness::NotLive
    }
}

/// Whether the process holding an exclusive `flock` on `lock` is running: Codex's thread writer
/// lock. A lock file that is not there, or that nothing holds, is no writer.
pub(crate) fn flock_holder(lock: &Path, procs: &Processes) -> Liveness {
    let file = match File::open(lock) {
        Ok(file) => file,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Liveness::NotLive,
        Err(_) => return Liveness::Unknown,
    };
    match file.try_lock() {
        // Held for as long as the handle lives: let go at once.
        Ok(()) => {
            let _ = file.unlock();
            Liveness::NotLive
        }
        Err(std::fs::TryLockError::WouldBlock) => Liveness::Live {
            pid: lock_owner(&file, procs),
        },
        Err(std::fs::TryLockError::Error(_)) => Liveness::Unknown,
    }
}

/// The process holding a `flock` on `file`, from `/proc/locks` (Linux only).
fn lock_owner(file: &File, procs: &Processes) -> Option<u32> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let Processes::ProcFs(root) = procs else {
            return None;
        };
        let inode = file.metadata().ok()?.ino();
        let locks = std::fs::read_to_string(root.join("locks")).ok()?;
        locks.lines().find_map(|line| {
            // `1: FLOCK  ADVISORY  WRITE 1592369 00:2a:918592 0 EOF`
            let fields: Vec<&str> = line.split_whitespace().collect();
            let (kind, pid, device) = (fields.get(1)?, fields.get(4)?, fields.get(5)?);
            let ino: u64 = device.rsplit(':').next()?.parse().ok()?;
            (*kind == "FLOCK" && ino == inode).then(|| pid.parse().ok()).flatten()
        })
    }
    #[cfg(not(unix))]
    {
        let _ = (file, procs);
        None
    }
}

#[cfg(test)]
mod tests;
