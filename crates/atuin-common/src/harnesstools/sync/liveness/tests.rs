use pretty_assertions::assert_eq;
use rstest::{fixture, rstest};
use tempfile::TempDir;

use super::*;

#[fixture]
fn dir() -> TempDir {
    tempfile::tempdir().unwrap()
}

/// A process directory under `root`, as `/proc` lays one out.
fn fake(root: &Path, pid: u32, cmdline: &[u8], comm: &str, state: &str, cwd: Option<&Path>) {
    let at = root.join(pid.to_string());
    std::fs::create_dir_all(&at).unwrap();
    std::fs::write(at.join("cmdline"), cmdline).unwrap();
    std::fs::write(at.join("comm"), format!("{comm}\n")).unwrap();
    let fields = vec!["0"; 18].join(" ");
    std::fs::write(at.join("stat"), format!("{pid} ({comm} x) {state} {fields} 4242 0")).unwrap();
    #[cfg(unix)]
    if let Some(cwd) = cwd {
        std::os::unix::fs::symlink(cwd, at.join("cwd")).unwrap();
    }
}

#[rstest]
fn a_proc_filesystem_is_read_as_linux_lays_it_out(dir: TempDir) {
    let root = dir.path().join("proc");
    // pi, whose title padded its command line with NULs over its arguments.
    fake(&root, 10, b"pi\0\0\0\0\0\0", "pi", "S", Some(dir.path()));
    fake(&root, 11, b"opencode\0--session\0ses_1\0", "opencode", "R", None);
    fake(&root, 12, b"", "defunct", "Z", None);
    std::fs::create_dir_all(root.join("self")).unwrap();
    std::fs::write(root.join("locks"), "").unwrap();

    let mut listed = Processes::ProcFs(root.clone()).list().unwrap();
    listed.sort_by_key(|p| p.pid);
    assert_eq!(listed, vec![
        ProcessInfo {
            pid: 10,
            argv: vec!["pi".to_owned()],
            name: Some("pi".to_owned()),
            cwd: Some(dir.path().to_owned()),
        },
        ProcessInfo {
            pid: 11,
            argv: vec!["opencode".to_owned(), "--session".to_owned(), "ses_1".to_owned()],
            name: Some("opencode".to_owned()),
            cwd: None,
        },
    ]);
    let procs = Processes::ProcFs(root);
    assert_eq!(
        procs.running(10),
        Some(Running {
            start: Some("4242".to_owned())
        })
    );
    assert_eq!(procs.running(12), None, "a zombie runs nothing");
    assert_eq!(procs.running(13), None);
}

#[cfg(target_os = "linux")]
#[rstest]
fn this_process_is_running() {
    let procs = Processes::here();
    let me = procs.running(std::process::id()).unwrap();
    assert!(me.start.is_some());
    let listed = procs.list().unwrap();
    assert!(listed.iter().any(|p| p.pid == std::process::id()));
}

fn process(argv: &[&str]) -> ProcessInfo {
    ProcessInfo {
        pid: 5,
        argv: argv.iter().map(|a| (*a).to_owned()).collect(),
        name: None,
        cwd: Some(PathBuf::from("/work/proj")),
    }
}

#[rstest]
#[case::tui(&["opencode"], Claim::Directory(None))]
#[case::tui_on_it(&["/home/u/.opencode/bin/opencode", "--session", "ses_1"], Claim::Session(true))]
#[case::tui_on_it_short(&["opencode", "-s", "ses_1"], Claim::Session(true))]
#[case::tui_on_it_joined(&["opencode", "--session=ses_1"], Claim::Session(true))]
#[case::tui_on_another(&["opencode", "-s", "ses_2"], Claim::Session(false))]
#[case::tui_continuing(&["opencode", "--continue"], Claim::Directory(None))]
#[case::tui_in_a_project(&["opencode", "/other/proj"], Claim::Directory(Some("/other/proj".into())))]
#[case::run(&["opencode", "run", "-s", "ses_1", "hello"], Claim::Session(true))]
#[case::run_new(&["opencode", "run", "hello"], Claim::Directory(None))]
#[case::server(&["opencode", "serve", "--port", "4096"], Claim::Any)]
#[case::web(&["opencode", "--log-level", "INFO", "web"], Claim::Any)]
#[case::export(&["opencode", "export", "ses_1"], Claim::None)]
#[case::import(&["opencode", "import", "--pure", "/tmp/x.json"], Claim::None)]
#[case::attach(&["opencode", "attach", "http://localhost:4096", "-s", "ses_1"], Claim::None)]
#[case::help(&["opencode", "--help"], Claim::None)]
#[case::dev_checkout(&["bun", "/src/opencode", "-s", "ses_1"], Claim::Session(true))]
#[case::not_opencode(&["opencode-helper", "-s", "ses_1"], Claim::None)]
fn opencode_processes_claim_what_their_command_line_says(
    #[case] argv: &[&str],
    #[case] expected: Claim,
) {
    assert_eq!(opencode_claim(&process(argv), "ses_1"), expected);
}

#[rstest]
fn a_project_argument_is_where_the_tui_works() {
    let tui = process(&["opencode", "sub"]);
    let cwd = |dir: &str| {
        scan(std::slice::from_ref(&tui), Some(Path::new(dir)), |p| opencode_claim(p, "ses_1"))
    };
    assert_eq!(cwd("/work/proj/sub"), Liveness::Unknown);
    assert_eq!(cwd("/work/proj"), Liveness::NotLive);
}

#[rstest]
#[case::named(vec![process(&["opencode", "-s", "ses_1"])], Some("/elsewhere"), Liveness::Live { pid: Some(5) })]
#[case::named_before_a_server(
    vec![process(&["opencode", "serve"]), process(&["opencode", "-s", "ses_1"])],
    Some("/elsewhere"),
    Liveness::Live { pid: Some(5) },
)]
#[case::a_server(vec![process(&["opencode", "serve"])], Some("/elsewhere"), Liveness::Unknown)]
#[case::same_directory(vec![process(&["opencode"])], Some("/work/proj"), Liveness::Unknown)]
#[case::another_directory(vec![process(&["opencode"])], Some("/elsewhere"), Liveness::NotLive)]
#[case::unknown_directory(vec![process(&["opencode"])], None, Liveness::Unknown)]
#[case::nothing(vec![], Some("/work/proj"), Liveness::NotLive)]
fn a_scan_is_live_on_a_name_and_unknown_on_a_doubt(
    #[case] processes: Vec<ProcessInfo>,
    #[case] cwd: Option<&str>,
    #[case] expected: Liveness,
) {
    assert_eq!(scan(&processes, cwd.map(Path::new), |p| opencode_claim(p, "ses_1")), expected);
}

#[rstest]
fn a_process_whose_directory_cannot_be_read_is_unknown() {
    let mut hidden = process(&["opencode"]);
    hidden.cwd = None;
    assert_eq!(
        scan(&[hidden], Some(Path::new("/work/proj")), |p| opencode_claim(p, "ses_1")),
        Liveness::Unknown
    );
}

/// A held `flock` is a writer (another open file description conflicts even in this process),
/// and the lock's holder is found in `/proc/locks`.
#[rstest]
fn a_held_flock_is_a_writer(dir: TempDir) {
    let lock = dir.path().join("t.lock");
    std::fs::write(&lock, "").unwrap();
    let procs = Processes::here();
    assert_eq!(flock_holder(&lock, &procs), Liveness::NotLive);

    let held = File::open(&lock).unwrap();
    held.lock().unwrap();
    let expected = if cfg!(target_os = "linux") {
        Some(std::process::id())
    } else {
        None
    };
    assert_eq!(flock_holder(&lock, &procs), Liveness::Live { pid: expected });
    drop(held);
    assert_eq!(flock_holder(&lock, &procs), Liveness::NotLive);
}

#[rstest]
fn no_lock_file_is_no_writer(dir: TempDir) {
    assert_eq!(flock_holder(&dir.path().join("none.lock"), &Processes::here()), Liveness::NotLive);
}
