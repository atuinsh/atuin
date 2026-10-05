//! Whether a harness process on this machine may be writing a session.
//!
//! - **Claude Code** registers every running session in `<config dir>/sessions/<pid>.json`
//!   (see the `ccode` module), checked against the running processes here.
//! - **Codex** holds an exclusive `flock` on `$CODEX_HOME/thread-writer-locks/<thread>.lock` for
//!   as long as a process has the thread loaded ([`flock_holder`]).
//! - **opencode**, **pi**, and a Codex that keeps no locks record nothing, and none of them
//!   reliably says on its command line which session it has open: any process of the harness
//!   running here in one of the session's directories (or below one) may be writing it
//!   ([`agent_running`]: [`Liveness::Unknown`]). A process whose directory can't be read may be
//!   in one; for a session whose directories are not all known ([`Dirs::Any`]), any process of
//!   the harness may be writing it. A session can also be opened from elsewhere (`opencode -s
//!   <id>`, `codex resume <id>`, `pi --session <file>`), so wherever it runs, a process of the
//!   harness counts when its command line names the session ([`Seen::names`]), and any process
//!   of the harness at all counts while the session's transcript has changed in the last
//!   [`RECENT`] ([`Seen::changing`]): a transcript still changing is the plainest sign of a
//!   writer.
//!
//! Processes are read from `/proc` on Linux, and through `sysinfo` elsewhere (macOS
//! `proc_pidinfo` and `KERN_PROCARGS2`, Windows' process snapshot); where `sysinfo` reads none,
//! whatever would have been looked up in them is [`Liveness::Unknown`]. Linux keeps `/proc`:
//! `sysinfo` reads a field it can't read as empty, and its start times are seconds, where Claude
//! Code records `/proc`'s clock ticks. Reading fails closed: only a process that is not there
//! (`ENOENT`, or missing from the table and not signalable) is not running, and a process that is
//! there but can't be read may be any harness's.

use std::ffi::OsString;
use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Whether a harness process on this machine may be writing a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Liveness {
    /// A harness process has the session open: process `pid`, when that is known.
    Live {
        pid: Option<u32>,
    },
    /// Nothing on this machine has the session open.
    NotLive,
    /// A harness process may have the session open; callers take it as live.
    Unknown,
}

/// One running process, as much of it as can be read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProcessInfo {
    pub pid: u32,
    /// Its command line, as the process presents it now; `None` when it can't be read.
    pub argv: Option<Vec<String>>,
    /// Its name (Linux `comm`).
    pub name: Option<String>,
    /// Its working directory; `None` when it can't be read.
    pub cwd: Option<PathBuf>,
}

/// Where the processes of this machine are read from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Processes {
    /// A `proc` filesystem mounted at this path (Linux: `/proc`; tests: a directory laid out
    /// like one).
    ProcFs(PathBuf),
    /// The platform's own process table, through `sysinfo` (macOS, Windows, the BSDs).
    System,
    /// A process table read already, as `sysinfo` reads one (tests).
    Table(Vec<TableEntry>),
    /// None can be read on this platform.
    Unavailable,
}

/// One process as `sysinfo` reads it, before it is taken for a [`ProcessInfo`] or a
/// [`Running`]: what `sysinfo` can't read of a process, it reads as empty (or 0).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TableEntry {
    pub pid: u32,
    /// Its executable's file name (macOS), or its image name (Windows: `node.exe`).
    pub name: OsString,
    pub cmd: Vec<OsString>,
    /// When it started, in seconds since the Unix epoch.
    pub start_time: u64,
    pub zombie: bool,
    /// Its working directory, `None` (or empty) when it can't be read.
    pub cwd: Option<PathBuf>,
}

/// When a process started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Start {
    /// Linux: the `starttime` field of `/proc/<pid>/stat` (clock ticks after boot), as written
    /// there and as Claude Code records it.
    BootTicks(String),
    /// Seconds since the Unix epoch, as `sysinfo` reads it.
    Epoch(u64),
}

/// A running process, and when it started, when that can be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Running {
    pub start: Option<Start>,
}

impl Processes {
    /// This machine's processes.
    #[must_use]
    pub fn here() -> Self {
        if cfg!(target_os = "linux") {
            Self::ProcFs(PathBuf::from("/proc"))
        } else if sysinfo::IS_SUPPORTED_SYSTEM {
            Self::System
        } else {
            Self::Unavailable
        }
    }

    /// Every process that can be seen; `Err` when they can't be read.
    pub fn list(&self) -> io::Result<Vec<ProcessInfo>> {
        match self {
            Self::ProcFs(root) => list_proc_fs(root),
            Self::System => Ok(read_table(None)?.iter().filter_map(TableEntry::info).collect()),
            Self::Table(table) => Ok(table.iter().filter_map(TableEntry::info).collect()),
            Self::Unavailable => Err(io::Error::from(io::ErrorKind::Unsupported)),
        }
    }

    /// Process `pid`, if it is running; `Err` when that can't be told: only a process that is
    /// not there (or a zombie) is not running, and one that can't be read (whose `stat` can't
    /// be read or parsed) may be.
    pub fn running(&self, pid: u32) -> io::Result<Option<Running>> {
        match self {
            Self::ProcFs(root) => match proc_stat(&root.join(pid.to_string())) {
                Ok(Some(stat)) => Ok((!stat.zombie).then_some(Running {
                    start: stat.start.map(Start::BootTicks),
                })),
                Ok(None) => Err(io::Error::new(io::ErrorKind::InvalidData, "unreadable stat")),
                Err(err) if gone(&err) => Ok(None),
                Err(err) => Err(err),
            },
            Self::System => match read_table(Some(pid))?.iter().find(|e| e.pid == pid) {
                Some(entry) => Ok(entry.running()),
                // `sysinfo` leaves out a process it can't even name.
                None if signalable(pid) => {
                    Err(io::Error::new(io::ErrorKind::PermissionDenied, "unreadable process"))
                }
                None => Ok(None),
            },
            Self::Table(table) => {
                Ok(table.iter().find(|e| e.pid == pid).and_then(TableEntry::running))
            }
            Self::Unavailable => Err(io::Error::from(io::ErrorKind::Unsupported)),
        }
    }
}

impl TableEntry {
    /// The process as it is listed; `None` for a zombie. An empty command line or name is one
    /// that couldn't be read (on macOS, another user's process's command line).
    fn info(&self) -> Option<ProcessInfo> {
        if self.zombie {
            return None;
        }
        let name = self.name.to_string_lossy();
        let name = name.strip_suffix(".exe").or_else(|| name.strip_suffix(".EXE")).unwrap_or(&name);
        Some(ProcessInfo {
            pid: self.pid,
            argv: (!self.cmd.is_empty())
                .then(|| self.cmd.iter().map(|a| a.to_string_lossy().into_owned()).collect()),
            name: (!name.is_empty()).then(|| name.to_owned()),
            cwd: self.cwd.clone().filter(|cwd| !cwd.as_os_str().is_empty()),
        })
    }

    /// The process as it runs; `None` for a zombie.
    fn running(&self) -> Option<Running> {
        (!self.zombie).then(|| Running {
            start: (self.start_time != 0).then_some(Start::Epoch(self.start_time)),
        })
    }
}

/// This machine's process table, through `sysinfo`: every process, or process `pid` (and this
/// one). `Err` when it can't be read, which `sysinfo` tells as no processes at all: this one is
/// always there.
fn read_table(pid: Option<u32>) -> io::Result<Vec<TableEntry>> {
    use sysinfo::{Pid, ProcessRefreshKind, ProcessStatus, ProcessesToUpdate, UpdateKind};
    let me = Pid::from_u32(std::process::id());
    let some = pid.map(|pid| [Pid::from_u32(pid), me]);
    let (which, refresh) = match &some {
        Some(pids) => (ProcessesToUpdate::Some(pids), ProcessRefreshKind::nothing()),
        None => (
            ProcessesToUpdate::All,
            ProcessRefreshKind::nothing().with_cmd(UpdateKind::Always).with_cwd(UpdateKind::Always),
        ),
    };
    let mut system = sysinfo::System::new();
    system.refresh_processes_specifics(which, true, refresh);
    if system.process(me).is_none() {
        return Err(io::Error::other("the process table can't be read"));
    }
    Ok(system
        .processes()
        .values()
        .filter(|p| p.thread_kind().is_none())
        .map(|p| TableEntry {
            pid: p.pid().as_u32(),
            name: p.name().to_owned(),
            cmd: p.cmd().to_vec(),
            start_time: p.start_time(),
            zombie: p.status() == ProcessStatus::Zombie,
            cwd: p.cwd().map(Path::to_path_buf),
        })
        .collect())
}

/// Whether process `pid` is there to be signalled, if perhaps not by this one.
#[cfg(unix)]
fn signalable(pid: u32) -> bool {
    use crate::os::unix::process::{is_alive, pid_from_u32};
    pid_from_u32(pid).is_some_and(is_alive)
}

/// Windows' process snapshot lists every process, whoever runs it.
#[cfg(not(unix))]
const fn signalable(_pid: u32) -> bool {
    false
}

/// The processes of the `proc` filesystem at `root`.
fn list_proc_fs(root: &Path) -> io::Result<Vec<ProcessInfo>> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(root)? {
        // An entry that can't be listed may be any process.
        let entry = entry?;
        let Some(pid) = entry.file_name().to_str().and_then(|n| n.parse::<u32>().ok()) else {
            continue;
        };
        let dir = entry.path();
        // A process that exits while it is read is simply not there; one that is there but
        // can't be read is kept, unread.
        let argv = match std::fs::read(dir.join("cmdline")) {
            Ok(cmdline) => Some(split_cmdline(&cmdline)),
            Err(err) if gone(&err) => continue,
            Err(_) => None,
        };
        let name = match std::fs::read_to_string(dir.join("comm")) {
            Ok(name) => Some(name.trim_end().to_owned()),
            Err(err) if gone(&err) => continue,
            Err(_) => None,
        };
        match proc_stat(&dir) {
            Ok(Some(stat)) if stat.zombie => continue,
            Err(err) if gone(&err) => continue,
            _ => {}
        }
        out.push(ProcessInfo {
            pid,
            argv,
            name,
            cwd: proc_cwd(&dir),
        });
    }
    Ok(out)
}

/// `<dir>/cwd`, the process's working directory; `None` when it can't be read (another user's
/// process), which is never taken to mean that the process is gone. Linux marks a directory
/// removed since with ` (deleted)`.
fn proc_cwd(dir: &Path) -> Option<PathBuf> {
    let cwd = std::fs::read_link(dir.join("cwd")).ok()?;
    if cwd.as_os_str().is_empty() {
        return None;
    }
    match cwd.to_str().and_then(|c| c.strip_suffix(" (deleted)")) {
        Some(removed) if !cwd.exists() => Some(PathBuf::from(removed)),
        _ => Some(cwd),
    }
}

/// Whether reading a process failed because it is not there: it never was, or it has exited.
fn gone(err: &io::Error) -> bool {
    err.kind() == io::ErrorKind::NotFound
}

struct Stat {
    zombie: bool,
    start: Option<String>,
}

/// `<dir>/stat`: whether the process is a zombie, and its `starttime` (field 22), read after the
/// command name's closing parenthesis; `Ok(None)` when it reads as no `stat` at all.
fn proc_stat(dir: &Path) -> io::Result<Option<Stat>> {
    let stat = std::fs::read_to_string(dir.join("stat"))?;
    Ok(stat.rfind(')').map(|at| {
        let fields: Vec<&str> = stat[at + 1..].split_whitespace().collect();
        Stat {
            zombie: fields.first() == Some(&"Z"),
            start: fields.get(19).map(|s| (*s).to_owned()),
        }
    }))
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

/// The file name of `arg`, without an `.exe` or `.js`.
fn program_name(arg: &str) -> &str {
    let name = arg.rsplit(['/', '\\']).next().unwrap_or(arg);
    name.strip_suffix(".exe").or_else(|| name.strip_suffix(".js")).unwrap_or(name)
}

/// Interpreters a harness may run under as a script, whose process name says nothing of which.
const INTERPRETERS: &[&str] = &["node", "nodejs", "bun", "deno"];

/// Whether `process` may be a run of the harness called `name` (or one of its `aliases`): its
/// program, any argument of its interpreter (a node launcher: `node <script>`,
/// `bun run <script>`), or its process name (pi renames itself). What can't be read of it may be
/// any of these: a process whose command line can't be read may be the harness unless its
/// process name says otherwise (it reads, and is neither the harness's nor an interpreter's), and
/// one whose process name can't be read unless its command line names another program.
///
/// Under an interpreter, every argument past `argv[0]` that is not a flag counts, not just the
/// script: which of an interpreter's flags take their value as the next argument can't be known
/// (`node --env-file <file> <script>`), and guessing wrong would miss the harness. So an
/// unrelated script given the harness's name as an argument (`node server.js /tmp/opencode`)
/// counts too. That is the trade-off taken: counting an unrelated interpreter process in the
/// session's directory (which only holds the session back until it exits) over missing the
/// harness (a session read while it is written). [`agent_running`] still counts such a process
/// only where it may be writing the session: in its directory, with its command line naming the
/// session, or while its transcript is changing.
fn may_be_agent(process: &ProcessInfo, name: &str, aliases: &[&str]) -> bool {
    if process.name.as_deref() == Some(name) {
        return true;
    }
    let Some(argv) = &process.argv else {
        return process.name.as_deref().is_none_or(|comm| INTERPRETERS.contains(&comm));
    };
    let named = |arg: &String| program_name(arg) == name;
    let aliased = |arg: &String| aliases.iter().any(|alias| arg.contains(alias));
    let interpreted = argv
        .first()
        .map(|arg| program_name(arg))
        .is_some_and(|program| INTERPRETERS.contains(&program))
        || process.name.as_deref().is_some_and(|comm| INTERPRETERS.contains(&comm));
    // A program's own name, and then any argument of its interpreter, or a program's first
    // argument; flags name no harness.
    let args = argv.iter().skip(1).take(if interpreted {
        usize::MAX
    } else {
        1
    });
    let args = args.filter(|arg| !arg.starts_with('-'));
    if argv.first().into_iter().chain(args).any(|arg| named(arg) || aliased(arg)) {
        return true;
    }
    process.name.is_none() && argv.first().is_none_or(String::is_empty)
}

/// Whether `dir` is `root` or inside it: compared component-wise as written, and failing that,
/// both resolved (symlinks, `..`, a Windows path's case) where both can be.
fn within(dir: &Path, root: &Path) -> bool {
    if dir.starts_with(root) {
        return true;
    }
    match (dir.canonicalize(), root.canonicalize()) {
        (Ok(dir), Ok(root)) => dir.starts_with(root),
        _ => false,
    }
}

/// Where a session may be working, for telling which processes of its harness work there.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Dirs {
    /// Anywhere: where it works is not (all) known, so a process of the harness may be working
    /// on it wherever it works.
    #[default]
    Any,
    /// In any of these directories, or below one.
    Within(Vec<PathBuf>),
}

impl Dirs {
    /// The session last seen working in `cwd`, or anywhere when that is not known.
    #[must_use]
    pub fn of(cwd: Option<PathBuf>) -> Self {
        cwd.map_or(Self::Any, |cwd| Self::Within(vec![cwd]))
    }
}

/// What is known of a session, for telling which processes of its harness may be writing it
/// ([`agent_running`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Seen {
    /// Where the session may be working.
    pub dirs: Dirs,
    /// What a command line opening the session would name of it: its id, its file. A process
    /// whose command line holds any of these counts wherever it works.
    pub names: Vec<String>,
    /// Its transcript changed in the last [`RECENT`] (or when can't be told:
    /// [`changed_lately`]): every process of the harness counts, wherever it works.
    pub changing: bool,
}

/// How lately a transcript must have changed for every process of its harness, wherever it
/// works, to count as one that may be writing it: two minutes, longer than a harness goes
/// without writing anything while it works on a reply (each streamed part, tool call and result
/// is written as it comes), and short enough that a session left a while ago is caught up.
pub const RECENT: Duration = Duration::from_secs(120);

/// Whether a transcript last changed at `modified` changed in the last [`RECENT`]: also when
/// that can't be read (`None`), or is later than now.
#[must_use]
pub fn changed_lately(modified: Option<SystemTime>) -> bool {
    modified
        .is_none_or(|at| !matches!(SystemTime::now().duration_since(at), Ok(ago) if ago >= RECENT))
}

/// Whether `process` may be working on a session in `dirs`: it works in one of them or below
/// it, or where it works, or where the session does, can't be told.
fn may_work_in(process: &ProcessInfo, dirs: &Dirs) -> bool {
    match (&process.cwd, dirs) {
        (Some(dir), Dirs::Within(roots)) => roots.iter().any(|root| within(dir, root)),
        _ => true,
    }
}

/// Whether the command line of `process` names any of `names` (as an argument, or within one:
/// `--session=<id>`, a path to the session's file).
fn names_any(process: &ProcessInfo, names: &[String]) -> bool {
    let Some(argv) = &process.argv else {
        return false;
    };
    let names: Vec<&String> = names.iter().filter(|n| !n.is_empty()).collect();
    argv.iter().skip(1).any(|arg| names.iter().any(|name| arg.contains(name.as_str())))
}

/// [`Liveness::Unknown`] while any process of the harness called `name` (or one of its
/// `aliases`, such as a package name in a script's path) runs here that may be writing the
/// session `seen`: one working in one of the session's directories or below it (one whose
/// directory can't be read may be there, and with [`Dirs::Any`], any may), one whose command line
/// names the session, wherever it works, or, while the session's transcript is changing, any at
/// all. [`Liveness::NotLive`] when none does.
#[must_use]
pub fn agent_running(procs: &Processes, name: &str, aliases: &[&str], seen: &Seen) -> Liveness {
    let Ok(processes) = procs.list() else {
        return Liveness::Unknown;
    };
    let may_write =
        |p: &ProcessInfo| seen.changing || may_work_in(p, &seen.dirs) || names_any(p, &seen.names);
    if processes.iter().any(|p| may_be_agent(p, name, aliases) && may_write(p)) {
        Liveness::Unknown
    } else {
        Liveness::NotLive
    }
}

/// Whether a process holds an exclusive `flock` on `lock` (Codex's thread writer lock). A lock
/// file that is not there, or that nothing holds, is no writer.
#[must_use]
pub fn flock_holder(lock: &Path, procs: &Processes) -> Liveness {
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

/// The process holding a `flock` on `file`, from `<proc>/locks` (Linux only: elsewhere a held
/// lock is a writer whose pid is unknown).
#[cfg(unix)]
fn lock_owner(file: &File, procs: &Processes) -> Option<u32> {
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
const fn lock_owner(_file: &File, _procs: &Processes) -> Option<u32> {
    None
}

#[cfg(test)]
pub(super) mod tests;
