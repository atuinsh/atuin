use std::fs::File;

use pretty_assertions::assert_eq;
use rstest::{fixture, rstest};
use tempfile::TempDir;

use super::*;

mod append;

const THREAD: &str = "01a0ea43-08ee-7733-a4bd-7f4dc0189cda";

#[fixture]
fn home() -> TempDir {
    tempfile::tempdir().unwrap()
}

fn lock_dir(home: &TempDir) -> PathBuf {
    let locks = home.path().join("thread-writer-locks");
    std::fs::create_dir_all(&locks).unwrap();
    std::fs::write(locks.join(".coordination.lock"), "").unwrap();
    locks
}

/// No processes at all: only the lock may say anything.
fn nothing(home: &TempDir) -> Processes {
    running(home, &[])
}

/// A `/proc` under `home` listing one process (pid 9, in `/work/proj`) per command line.
fn running(home: &TempDir, commands: &[&[&str]]) -> Processes {
    let root = home.path().join("proc");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("locks"), "").unwrap();
    for (n, argv) in commands.iter().enumerate() {
        let pid = 9 + n;
        let at = root.join(pid.to_string());
        std::fs::create_dir_all(&at).unwrap();
        let cmdline: Vec<u8> = argv.iter().flat_map(|a| [a.as_bytes(), b"\0"].concat()).collect();
        std::fs::write(at.join("cmdline"), cmdline).unwrap();
        std::fs::write(at.join("comm"), "codex\n").unwrap();
        let fields = vec!["0"; 18].join(" ");
        std::fs::write(at.join("stat"), format!("{pid} (codex) S {fields} 4242 0")).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("/work/proj", at.join("cwd")).unwrap();
    }
    Processes::ProcFs(root)
}

#[rstest]
#[case::the_thread(THREAD)]
#[case::a_segment_of_it(&format!("{THREAD}_2"))]
fn the_thread_writer_lock_says_whether_a_codex_has_it_loaded(home: TempDir, #[case] id: &str) {
    let locks = lock_dir(&home);
    let procs = nothing(&home);
    assert_eq!(liveness(home.path(), id, None, &procs), Liveness::NotLive);

    let lock = locks.join(format!("{THREAD}.lock"));
    std::fs::write(&lock, "").unwrap();
    assert_eq!(liveness(home.path(), id, None, &procs), Liveness::NotLive, "left, not held");

    let held = File::open(&lock).unwrap();
    held.lock().unwrap();
    assert!(matches!(liveness(home.path(), id, None, &procs), Liveness::Live { .. }));
    drop(held);
    assert_eq!(liveness(home.path(), id, None, &procs), Liveness::NotLive);
}

#[rstest]
fn another_threads_lock_is_not_this_ones(home: TempDir) {
    let locks = lock_dir(&home);
    let held = File::create(locks.join("01a0ffff-0000-7000-8000-000000000000.lock")).unwrap();
    held.lock().unwrap();
    assert_eq!(liveness(home.path(), THREAD, None, &nothing(&home)), Liveness::NotLive);
}

fn codex(argv: &[&str]) -> ProcessInfo {
    ProcessInfo {
        pid: 9,
        argv: argv.iter().map(|a| (*a).to_owned()).collect(),
        name: None,
        cwd: Some(PathBuf::from("/work/proj")),
    }
}

/// A Codex that keeps no locks (no lock directory) is looked for among the processes.
#[rstest]
#[case::resuming_it(&["codex", "resume", THREAD], Liveness::Live { pid: Some(9) })]
#[case::exec_resuming_it(&["node", "/x/bin/codex.js", "exec", "resume", THREAD, "go"], Liveness::Live { pid: Some(9) })]
#[case::resuming_another(&["codex", "resume", "01a0ffff"], Liveness::NotLive)]
#[case::in_its_directory(&["codex"], Liveness::Unknown)]
#[case::an_app_server(&["codex", "app-server"], Liveness::Unknown)]
#[case::logging_in(&["codex", "login"], Liveness::NotLive)]
fn without_locks_codex_processes_are_looked_for(#[case] argv: &[&str], #[case] expected: Liveness) {
    let processes = [codex(argv)];
    let found = scan(&processes, Some(Path::new("/work/proj")), |p| claim(p, THREAD));
    assert_eq!(found, expected);
}

/// Without its lock file, a thread may still be loaded by a Codex that keeps no locks, whether
/// or not the lock directory is there (a newer Codex, or anything else, may have made it): its
/// processes are looked for, and a doubt keeps the thread from being written.
#[cfg(target_os = "linux")]
#[rstest]
#[case::no_lock_directory(false)]
#[case::a_lock_directory(true)]
fn without_its_lock_file_a_thread_is_looked_for_among_the_processes(
    home: TempDir,
    #[case] directory: bool,
) {
    if directory {
        lock_dir(&home);
    }
    let resuming = running(&home, &[&["codex", "resume", THREAD]]);
    assert_eq!(liveness(home.path(), THREAD, None, &resuming), Liveness::Live { pid: Some(9) });

    let home = tempfile::tempdir().unwrap();
    if directory {
        lock_dir(&home);
    }
    let serving = running(&home, &[&["codex", "app-server"]]);
    assert_eq!(liveness(home.path(), THREAD, None, &serving), Liveness::Unknown);
    assert!(require_idle(liveness(home.path(), THREAD, None, &serving)).is_err());
}

/// A held lock file says the thread is loaded, whatever the processes say.
#[cfg(unix)]
#[rstest]
fn a_held_lock_file_is_live_without_a_codex_process(home: TempDir) {
    let locks = lock_dir(&home);
    let held = File::create(locks.join(format!("{THREAD}.lock"))).unwrap();
    held.lock().unwrap();
    let procs = running(&home, &[&["codex", "resume", "01a0ffff"]]);
    assert!(matches!(liveness(home.path(), THREAD, None, &procs), Liveness::Live { .. }));
}
