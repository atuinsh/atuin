use pretty_assertions::assert_eq;
use rstest::{fixture, rstest};
use tempfile::TempDir;

use super::*;

#[fixture]
pub fn dir() -> TempDir {
    tempfile::tempdir().unwrap()
}

/// A process as a `/proc` of its own lays it out: `pid`, its command line (NUL-separated), its
/// `comm`, its state, its start time and its working directory (a `cwd` link, Unix only); and
/// which of its files (`cmdline`, `comm`, `stat`, `cwd`) are there but can't be read (laid out as
/// directories, which no one can read as a file or a link, root included).
pub struct Fake<'a> {
    pub pid: u32,
    pub cmdline: &'a [u8],
    pub comm: &'a str,
    pub state: &'a str,
    pub start: &'a str,
    pub unreadable: &'a [&'a str],
    pub cwd: Option<&'a Path>,
}

impl<'a> Fake<'a> {
    pub const fn running(pid: u32, cmdline: &'a [u8], start: &'a str) -> Self {
        Self {
            pid,
            cmdline,
            comm: "node",
            state: "S",
            start,
            unreadable: &[],
            cwd: None,
        }
    }
}

/// A `/proc` under `dir` holding `processes`.
pub fn procs(dir: &Path, processes: &[Fake<'_>]) -> Processes {
    let root = dir.join("proc");
    std::fs::create_dir_all(&root).unwrap();
    for p in processes {
        let at = root.join(p.pid.to_string());
        std::fs::create_dir_all(&at).unwrap();
        let fields = vec!["0"; 18].join(" ");
        let stat = format!("{} ({} x) {} {fields} {} 0", p.pid, p.comm, p.state, p.start);
        let files = [
            ("cmdline", p.cmdline.to_vec()),
            ("comm", format!("{}\n", p.comm).into_bytes()),
            ("stat", stat.into_bytes()),
        ];
        for (file, content) in files {
            if p.unreadable.contains(&file) {
                std::fs::create_dir(at.join(file)).unwrap();
            } else {
                std::fs::write(at.join(file), content).unwrap();
            }
        }
        if p.unreadable.contains(&"cwd") {
            std::fs::create_dir(at.join("cwd")).unwrap();
        } else if let Some(cwd) = p.cwd {
            #[cfg(unix)]
            std::os::unix::fs::symlink(cwd, at.join("cwd")).unwrap();
            #[cfg(not(unix))]
            let _ = cwd;
        }
    }
    Processes::ProcFs(root)
}

#[rstest]
fn a_proc_filesystem_is_read_as_linux_lays_it_out(dir: TempDir) {
    let procs = procs(dir.path(), &[
        // pi, whose title padded its command line with NULs over its arguments.
        Fake {
            comm: "pi",
            ..Fake::running(10, b"pi\0\0\0\0\0\0", "4242")
        },
        Fake::running(11, b"opencode\0--session\0ses_1\0", "1"),
        Fake {
            state: "Z",
            ..Fake::running(12, b"", "1")
        },
    ]);
    let mut listed = procs.list().unwrap();
    listed.sort_by_key(|p| p.pid);
    let process = |pid, argv: &[&str], name: &str| ProcessInfo {
        pid,
        argv: Some(argv.iter().map(|a| (*a).to_owned()).collect()),
        name: Some(name.to_owned()),
        cwd: None,
    };
    assert_eq!(listed, vec![
        process(10, &["pi"], "pi"),
        process(11, &["opencode", "--session", "ses_1"], "node"),
    ]);
    let running = Running {
        start: Some(Start::BootTicks("4242".to_owned())),
    };
    assert_eq!(procs.running(10).unwrap(), Some(running));
    assert_eq!(procs.running(12).unwrap(), None, "a zombie runs nothing");
    assert_eq!(procs.running(13).unwrap(), None);
}

#[rstest]
fn a_process_that_cant_be_read_is_listed_unread(dir: TempDir) {
    let procs = procs(dir.path(), &[
        Fake {
            unreadable: &["cmdline"],
            ..Fake::running(20, b"opencode\0", "1")
        },
        Fake {
            unreadable: &["comm", "stat"],
            ..Fake::running(21, b"opencode\0", "1")
        },
    ]);
    let mut listed = procs.list().unwrap();
    listed.sort_by_key(|p| p.pid);
    assert_eq!(listed, vec![
        ProcessInfo {
            pid: 20,
            argv: None,
            name: Some("node".to_owned()),
            cwd: None,
        },
        ProcessInfo {
            pid: 21,
            argv: Some(vec!["opencode".to_owned()]),
            name: None,
            cwd: None,
        },
    ]);
}

/// Only a process that is not there is not running: one whose `stat` is there but can't be read
/// (or parsed) may be.
#[rstest]
#[case::readable(&[], Some(true))]
#[case::unreadable(&["stat"], None)]
#[case::its_command_line_unreadable(&["cmdline", "comm"], Some(true))]
fn a_running_process_whose_stat_cant_be_read_is_unknown(
    dir: TempDir,
    #[case] unreadable: &[&str],
    #[case] running: Option<bool>,
) {
    let procs = procs(dir.path(), &[Fake {
        unreadable,
        ..Fake::running(30, b"claude\0", "7")
    }]);
    assert_eq!(procs.running(30).ok().map(|r| r.is_some()), running);
    assert_eq!(procs.running(31).unwrap(), None, "not there");

    std::fs::create_dir_all(dir.path().join("proc/32")).unwrap();
    std::fs::write(dir.path().join("proc/32/stat"), "garbled").unwrap();
    assert!(procs.running(32).is_err(), "a stat that can't be parsed");
}

/// A process whose identity can't be read may be the agent, unless what can be read of it says
/// it is not.
#[rstest]
#[case::no_command_line_named_the_agent(b"".as_slice(), "opencode", &["cmdline"], Liveness::Unknown)]
#[case::no_command_line_under_an_interpreter(b"".as_slice(), "node", &["cmdline"], Liveness::Unknown)]
#[case::nothing_readable(b"".as_slice(), "bash", &["cmdline", "comm"], Liveness::Unknown)]
#[case::no_command_line_another_program(b"".as_slice(), "bash", &["cmdline"], Liveness::NotLive)]
#[case::no_name_the_agent(b"opencode\0".as_slice(), "node", &["comm"], Liveness::Unknown)]
#[case::no_name_another_program(b"vim\0x\0".as_slice(), "vim", &["comm"], Liveness::NotLive)]
#[case::no_name_nor_command_line(b"\0\0".as_slice(), "pi", &["comm"], Liveness::Unknown)]
#[case::no_stat_the_agent(b"opencode\0".as_slice(), "node", &["stat"], Liveness::Unknown)]
#[case::no_stat_another_program(b"vim\0".as_slice(), "vim", &["stat"], Liveness::NotLive)]
fn a_process_that_cant_be_read_may_be_the_agent(
    dir: TempDir,
    #[case] cmdline: &[u8],
    #[case] comm: &str,
    #[case] unreadable: &[&str],
    #[case] expected: Liveness,
) {
    let procs = procs(dir.path(), &[Fake {
        comm,
        unreadable,
        ..Fake::running(7, cmdline, "1")
    }]);
    assert_eq!(agent_running(&procs, "opencode", &[], &Seen::default()), expected);
}

/// Where a process works is its `cwd` link; one that can't be read is no sign the process is
/// gone.
#[cfg(unix)]
#[rstest]
fn a_proc_filesystem_tells_where_a_process_works(dir: TempDir) {
    let work = dir.path().join("work");
    std::fs::create_dir_all(&work).unwrap();
    let procs = procs(dir.path(), &[
        Fake {
            cwd: Some(&work),
            ..Fake::running(40, b"opencode\0", "1")
        },
        Fake {
            unreadable: &["cwd"],
            ..Fake::running(41, b"opencode\0", "1")
        },
    ]);
    let mut listed = procs.list().unwrap();
    listed.sort_by_key(|p| p.pid);
    let cwds: Vec<_> = listed.iter().map(|p| (p.pid, p.cwd.clone())).collect();
    assert_eq!(cwds, vec![(40, Some(work)), (41, None)]);
}

/// A process of the harness may be writing a session only where the session works, or below it;
/// one whose directory can't be read (or is read empty) may be anywhere, and for a session whose
/// directory is not known, every one may be writing it.
#[rstest]
#[case::the_sessions_directory(Some("proj"), Some("proj"), Liveness::Unknown)]
#[case::below_it(Some("proj/sub"), Some("proj"), Liveness::Unknown)]
#[case::the_same_once_resolved(Some("other/../proj/sub"), Some("proj"), Liveness::Unknown)]
#[case::another_directory(Some("other"), Some("proj"), Liveness::NotLive)]
#[case::a_sibling_sharing_its_prefix(Some("proj2"), Some("proj"), Liveness::NotLive)]
#[case::above_it(Some(""), Some("proj"), Liveness::NotLive)]
#[case::its_directory_unread(None, Some("proj"), Liveness::Unknown)]
#[case::its_directory_read_empty(Some("<empty>"), Some("proj"), Liveness::Unknown)]
#[case::the_sessions_directory_unknown(Some("other"), None, Liveness::Unknown)]
fn a_harness_counts_only_where_the_session_works(
    dir: TempDir,
    #[case] works_in: Option<&str>,
    #[case] session: Option<&str>,
    #[case] expected: Liveness,
) {
    for sub in ["proj/sub", "proj2", "other"] {
        std::fs::create_dir_all(dir.path().join(sub)).unwrap();
    }
    let at = |p: &str| {
        if p == "<empty>" {
            PathBuf::new()
        } else {
            dir.path().join(p)
        }
    };
    let procs = table(&[TableEntry {
        cwd: works_in.map(at),
        ..entry(7, "node", &["node", "/opt/lib/opencode"], 1)
    }]);
    let session = session.map(at);
    assert_eq!(agent_running(&procs, "opencode", &[], &cwd(session)), expected);
}

/// A session that may be working in any of several directories: a process of the harness in
/// (or below) any of them counts, and one elsewhere does not.
#[rstest]
#[case::the_first("a/sub", Liveness::Unknown)]
#[case::the_second("b", Liveness::Unknown)]
#[case::neither("c", Liveness::NotLive)]
fn a_harness_counts_in_any_of_the_sessions_directories(
    dir: TempDir,
    #[case] works_in: &str,
    #[case] expected: Liveness,
) {
    for sub in ["a/sub", "b", "c"] {
        std::fs::create_dir_all(dir.path().join(sub)).unwrap();
    }
    let procs = table(&[TableEntry {
        cwd: Some(dir.path().join(works_in)),
        ..entry(7, "opencode", &["opencode"], 1)
    }]);
    let seen = Seen {
        dirs: Dirs::Within(vec![dir.path().join("a"), dir.path().join("b")]),
        ..Seen::default()
    };
    assert_eq!(agent_running(&procs, "opencode", &[], &seen), expected);
}

/// A session opened from outside its directory: a process of the harness elsewhere counts when
/// its command line names the session, or while the session's transcript is changing.
/// An unrelated script under an interpreter, taken for the harness for naming it as an argument,
/// counts no more than the harness would.
#[rstest]
#[case::named_by_opencode_s("opencode", &["opencode", "-s", "ses_1"], false, Liveness::Unknown)]
#[case::named_by_session_eq("opencode", &["opencode", "--session=ses_1"], false, Liveness::Unknown)]
#[case::named_by_codex_resume("codex", &["codex", "resume", "ses_1"], false, Liveness::Unknown)]
#[case::named_by_its_file("pi", &["pi", "--session", "s/1_ses_1.jsonl"], false, Liveness::Unknown)]
#[case::named_under_node("opencode", &["node", "opencode", "-s", "ses_1"], false, Liveness::Unknown)]
#[case::another_session_named("opencode", &["opencode", "-s", "ses_2"], false, Liveness::NotLive)]
#[case::nothing_named("opencode", &["opencode"], false, Liveness::NotLive)]
#[case::nothing_named_but_changing("opencode", &["opencode"], true, Liveness::Unknown)]
#[case::another_program_naming_it("opencode", &["vim", "ses_1"], true, Liveness::NotLive)]
#[case::an_unrelated_interpreter_naming_the_harness("opencode", &["node", "server.js", "/tmp/opencode"], false, Liveness::NotLive)]
fn a_harness_elsewhere_counts_when_named_or_changing(
    dir: TempDir,
    #[case] harness: &str,
    #[case] cmd: &[&str],
    #[case] changing: bool,
    #[case] expected: Liveness,
) {
    let procs = table(&[TableEntry {
        cwd: Some(dir.path().join("other")),
        ..entry(7, cmd[0], cmd, 1)
    }]);
    let seen = Seen {
        dirs: Dirs::of(Some(dir.path().join("proj"))),
        names: vec!["ses_1".to_owned(), String::new()],
        changing,
    };
    assert_eq!(agent_running(&procs, harness, &[], &seen), expected);
}

/// A transcript changed in the last [`RECENT`] is changing; one whose change time can't be read,
/// or is later than now, may be.
#[rstest]
#[case::ten_seconds_ago(Some(-10), true)]
#[case::just_under_the_window(Some(-110), true)]
#[case::just_over_the_window(Some(-130), false)]
#[case::an_hour_ago(Some(-3600), false)]
#[case::in_the_future(Some(60), true)]
#[case::unreadable(None, true)]
fn a_recent_change_is_changing(#[case] secs: Option<i64>, #[case] expected: bool) {
    let at = secs.map(|s| {
        let now = SystemTime::now();
        let by = Duration::from_secs(s.unsigned_abs());
        if s < 0 {
            now - by
        } else {
            now + by
        }
    });
    assert_eq!(changed_lately(at), expected);
}

#[rstest]
fn processes_that_cant_be_read_are_unknown() {
    assert!(Processes::Unavailable.list().is_err());
    assert!(Processes::Unavailable.running(1).is_err());
    assert_eq!(
        agent_running(&Processes::Unavailable, "pi", &[], &Seen::default()),
        Liveness::Unknown
    );
}

/// A session last seen working in `cwd`, named by nothing, its transcript not changing.
pub fn cwd(cwd: Option<PathBuf>) -> Seen {
    Seen {
        dirs: Dirs::of(cwd),
        ..Seen::default()
    }
}

/// A process as `sysinfo` reads it: `pid`, its name, its command line, and when it started.
pub fn entry(pid: u32, name: &str, cmd: &[&str], start_time: u64) -> TableEntry {
    TableEntry {
        pid,
        name: name.into(),
        cmd: cmd.iter().map(Into::into).collect(),
        start_time,
        zombie: false,
        cwd: None,
    }
}

/// A process table as `sysinfo` reads one (macOS, Windows).
pub fn table(entries: &[TableEntry]) -> Processes {
    Processes::Table(entries.to_vec())
}

#[rstest]
fn a_process_table_is_read_as_sysinfo_reads_it() {
    let procs = table(&[
        entry(10, "node", &["node", "/opt/pi-coding-agent/dist/cli.js"], 1_791_203_696),
        // Windows names a process by its image.
        entry(11, "codex.exe", &["C:\\bin\\codex.exe", "resume"], 1_791_203_697),
        // Another user's process on macOS: its command line can't be read.
        entry(12, "node", &[], 1_791_203_698),
        // Nor its start time, nor its name.
        entry(13, "", &["opencode"], 0),
        TableEntry {
            zombie: true,
            ..entry(14, "codex", &["codex"], 1)
        },
    ]);
    let process = |pid, argv: Option<&[&str]>, name: Option<&str>| ProcessInfo {
        pid,
        argv: argv.map(|argv| argv.iter().map(|a| (*a).to_owned()).collect()),
        name: name.map(str::to_owned),
        cwd: None,
    };
    let mut listed = procs.list().unwrap();
    listed.sort_by_key(|p| p.pid);
    assert_eq!(listed, vec![
        process(10, Some(&["node", "/opt/pi-coding-agent/dist/cli.js"]), Some("node")),
        process(11, Some(&["C:\\bin\\codex.exe", "resume"]), Some("codex")),
        process(12, None, Some("node")),
        process(13, Some(&["opencode"]), None),
    ]);
    let started = |at| Some(Running { start: at });
    assert_eq!(procs.running(10).unwrap(), started(Some(Start::Epoch(1_791_203_696))));
    assert_eq!(procs.running(13).unwrap(), started(None), "no start time read");
    assert_eq!(procs.running(14).unwrap(), None, "a zombie runs nothing");
    assert_eq!(procs.running(15).unwrap(), None, "not there");
}

/// A process table tells whether the harness runs, as `/proc` does.
#[rstest]
#[case::none_running(&["bash"], "bash", Liveness::NotLive)]
#[case::the_harness(&["/usr/local/bin/opencode", "run"], "opencode", Liveness::Unknown)]
#[case::its_command_line_unreadable(&[], "node", Liveness::Unknown)]
#[case::another_program_unread(&[], "bash", Liveness::NotLive)]
fn a_process_table_tells_whether_the_harness_runs(
    #[case] cmd: &[&str],
    #[case] name: &str,
    #[case] expected: Liveness,
) {
    let procs = table(&[entry(7, name, cmd, 1_791_203_696)]);
    assert_eq!(agent_running(&procs, "opencode", &[], &Seen::default()), expected);
}

#[rstest]
#[case::the_program(b"/usr/local/bin/opencode\0run\0".as_slice(), "node", "opencode", true)]
#[case::a_windows_program(b"C:\\bin\\codex.exe\0".as_slice(), "node", "codex", true)]
#[case::a_node_launcher(b"node\0/opt/lib/codex.js\0resume\0".as_slice(), "node", "codex", true)]
#[case::by_its_process_name(b"\0\0\0".as_slice(), "pi", "pi", true)]
#[case::by_its_package(b"node\0/opt/pi-coding-agent/dist/cli.js\0".as_slice(), "node", "pi", true)]
#[case::another_program(b"vim\0opencode.md\0".as_slice(), "vim", "opencode", false)]
#[case::a_name_it_merely_contains(b"/bin/opencode-helper\0".as_slice(), "node", "opencode", false)]
#[case::a_script_bun_runs(b"bun\0run\0/path/to/opencode\0".as_slice(), "bun", "opencode", true)]
#[case::a_script_after_flags(b"node\0--flag\0/path/pi-coding-agent/cli.js\0".as_slice(), "node", "pi", true)]
#[case::a_script_after_flags_by_name(b"/usr/bin/node\0--enable-source-maps\0--no-warnings\0/opt/lib/codex.js\0".as_slice(), "MainThread", "codex", true)]
#[case::a_windows_interpreter(b"C:\\node.exe\0--inspect\0C:\\lib\\opencode\0".as_slice(), "x", "opencode", true)]
#[case::another_interpreted_script(b"node\0--flag\0/srv/app/server.js\0serve\0".as_slice(), "node", "opencode", false)]
#[case::a_flag_naming_the_agent(b"node\0--title=opencode\0/srv/app.js\0".as_slice(), "node", "opencode", false)]
#[case::an_argument_of_another_script(b"node\0server.js\0/tmp/opencode\0".as_slice(), "node", "opencode", true)]
#[case::an_argument_of_a_script_bun_runs(b"bun\0run\0app.ts\0opencode\0".as_slice(), "bun", "opencode", true)]
#[case::an_argument_of_a_script_by_name(b"/usr/bin/node\0app.js\0/opt/pi-coding-agent/x\0".as_slice(), "MainThread", "pi", true)]
#[case::a_required_module(b"node\0-r\0/opt/lib/opencode\0/srv/app.js\0".as_slice(), "node", "opencode", true)]
#[case::a_script_after_an_unknown_valued_flag(b"node\0--env-file\0/tmp/dev.env\0/opt/lib/opencode.js\0".as_slice(), "node", "opencode", true)]
#[case::a_script_after_a_required_module(b"node\0--require\0ts-node/register\0/opt/lib/opencode\0".as_slice(), "node", "opencode", true)]
#[case::a_script_deno_runs(b"deno\0run\0-A\0/opt/lib/opencode\0".as_slice(), "deno", "opencode", true)]
#[case::past_a_programs_first_argument(b"vim\0-o\0/tmp/opencode\0".as_slice(), "vim", "opencode", false)]
fn any_process_of_the_agent_may_be_writing(
    dir: TempDir,
    #[case] cmdline: &[u8],
    #[case] comm: &str,
    #[case] agent: &str,
    #[case] found: bool,
) {
    let procs = procs(dir.path(), &[Fake {
        comm,
        ..Fake::running(7, cmdline, "1")
    }]);
    let expected = if found {
        Liveness::Unknown
    } else {
        Liveness::NotLive
    };
    assert_eq!(agent_running(&procs, agent, &["pi-coding-agent"], &Seen::default()), expected);
}

#[rstest]
fn a_held_flock_is_a_writer(dir: TempDir) {
    let procs = procs(dir.path(), &[]);
    let lock = dir.path().join("thread.lock");
    assert_eq!(flock_holder(&lock, &procs), Liveness::NotLive, "no lock file");
    std::fs::write(&lock, "").unwrap();
    assert_eq!(flock_holder(&lock, &procs), Liveness::NotLive, "held by nothing");

    let held = File::open(&lock).unwrap();
    held.lock().unwrap();
    // This process holds it: its `/proc/locks` line names it.
    #[cfg(unix)]
    let pid = {
        use std::os::unix::fs::MetadataExt;
        let ino = held.metadata().unwrap().ino();
        let line = format!("1: FLOCK  ADVISORY  WRITE 4321 00:2a:{ino} 0 EOF\n");
        std::fs::write(dir.path().join("proc/locks"), line).unwrap();
        Some(4321)
    };
    #[cfg(not(unix))]
    let pid = None;
    assert_eq!(flock_holder(&lock, &procs), Liveness::Live { pid });
}
