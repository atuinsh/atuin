use std::collections::HashMap;

use atuin_client::ai_session::HarnessKind;
use atuin_common::harnesstools::resume::CwdRequirement;
use rstest::{fixture, rstest};
use tempfile::TempDir;

use super::*;
use crate::resume_tui::fake;

/// A machine that touches nothing: transcripts are found where `found` says, and every program
/// is installed unless `missing_programs` says otherwise.
#[derive(Clone, Default)]
struct FakeMachine {
    found: HashMap<String, PathBuf>,
    missing_programs: Vec<&'static str>,
}

#[async_trait]
impl Machine for FakeMachine {
    async fn locate(&self, _: AnyHarness, id: &str) -> Option<PathBuf> {
        self.found.get(id).cloned()
    }

    fn installed(&self, program: &str) -> bool {
        !self.missing_programs.contains(&program)
    }
}

/// A scratch filesystem: `here` (the current directory) and `elsewhere`, a directory that
/// exists.
struct Dirs {
    _tmp: TempDir,
    here: PathBuf,
    elsewhere: PathBuf,
}

#[fixture]
fn dirs() -> Dirs {
    let tmp = tempfile::tempdir().unwrap();
    let here = tmp.path().join("here");
    let elsewhere = tmp.path().join("elsewhere");
    for dir in [&here, &elsewhere] {
        std::fs::create_dir_all(dir).unwrap();
    }
    Dirs {
        _tmp: tmp,
        here,
        elsewhere,
    }
}

fn resumer(machine: &FakeMachine) -> HarnessResumer {
    HarnessResumer::on(AiSessionResume::default(), machine.clone())
}

fn row(harness: HarnessKind, id: &str, cwd: &Path, remote: bool) -> SessionRow {
    let mut row = fake::row(harness, id, "t");
    row.cwd = Some(cwd.to_owned());
    if remote {
        row.host_id = "other".to_owned();
    }
    row
}

#[rstest]
fn shell_line_quotes_the_directory() {
    let plan = ResumePlan {
        program: "claude".to_owned(),
        args: vec!["--resume".to_owned(), "abc".to_owned()],
        cwd: Some(PathBuf::from("/tmp/it's here")),
        cwd_requirement: CwdRequirement::Preferred,
        native_path: None,
    };
    assert_eq!(shell_line(&plan), r"cd -- '/tmp/it'\''s here' && claude --resume abc");
}

/// A session whose transcript is here resumes from it, as recorded.
#[rstest]
#[tokio::test]
async fn a_session_on_disk_resumes_directly(dirs: Dirs) {
    let machine = FakeMachine {
        found: HashMap::from([("abc-123".to_owned(), PathBuf::from("/t/abc-123.jsonl"))]),
        ..FakeMachine::default()
    };
    let local = row(HarnessKind::Pi, "abc-123", &dirs.elsewhere, false);
    let plan = resumer(&machine).plan(&local).await.unwrap();
    assert_eq!(plan.args, ["--session", "/t/abc-123.jsonl"]);
    assert_eq!(plan.cwd.as_deref(), Some(dirs.elsewhere.as_path()));
}

/// A session whose transcript isn't here (another host's, or deleted) can't be resumed.
#[rstest]
#[case::another_hosts(true)]
#[case::deleted(false)]
#[tokio::test]
async fn a_session_whose_transcript_isnt_here_cant_resume(dirs: Dirs, #[case] remote: bool) {
    let session = row(HarnessKind::ClaudeCode, "abc-123", &dirs.elsewhere, remote);
    let err = resumer(&FakeMachine::default()).plan(&session).await.unwrap_err();
    assert_eq!(err, NotResumable::NotHere);
    assert_eq!(err.to_string(), "its transcript isn't on this machine");
}

#[rstest]
#[tokio::test]
async fn a_harness_not_installed_here_says_so(dirs: Dirs) {
    let machine = FakeMachine {
        found: HashMap::from([("abc-123".to_owned(), PathBuf::from("/t/abc-123.jsonl"))]),
        missing_programs: vec!["claude"],
    };
    let local = row(HarnessKind::ClaudeCode, "abc-123", &dirs.elsewhere, false);
    let err = resumer(&machine).plan(&local).await.unwrap_err();
    assert_eq!(err, NotResumable::NotInstalled("claude".to_owned()));
    assert_eq!(err.to_string(), "`claude` isn't installed here (not found on PATH)");
}

#[rstest]
#[tokio::test]
async fn unsupported_sessions_and_subagents_are_not_resumable(dirs: Dirs) {
    let machine = FakeMachine::default();
    let resumer = resumer(&machine);
    let copilot = row(HarnessKind::Copilot, "cp-1", &dirs.here, false);
    assert_eq!(resumer.plan(&copilot).await.unwrap_err(), NotResumable::Unsupported("Copilot"));

    // A Claude Code subagent is refused by the harness before anything is looked up.
    let subagent = row(HarnessKind::ClaudeCode, "agent-a1b2", &dirs.here, true);
    assert!(matches!(
        resumer.plan(&subagent).await.unwrap_err(),
        NotResumable::Harness(ResumeError::NotResumable(_))
    ));
}

#[rstest]
fn finds_programs_on_path() {
    assert!(!on_path("definitely-not-a-program-atuin"));
}

#[cfg(unix)]
#[rstest]
fn finds_programs_on_path_and_by_path_on_unix() {
    assert!(on_path("sh"));
    assert!(find_program("sh").is_some_and(|path| path.is_absolute() && path.ends_with("sh")));
    assert!(on_path("/bin/sh"));
    assert!(!on_path("/nonexistent/sh"));

    // A file that isn't executable can't be run.
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("not-executable");
    std::fs::write(&file, "").unwrap();
    assert!(!on_path(file.to_str().unwrap()));
}

#[cfg(windows)]
#[rstest]
fn finds_programs_on_path_and_by_path_on_windows() {
    // `cmd` is found as `cmd.exe` through PATHEXT, on PATH or by its path.
    assert!(on_path("cmd"));
    assert!(on_path("cmd.exe"));
    // What runs is the file the check found, extension and all, which spawning `cmd` would not
    // resolve to were it a `.cmd` or `.bat` script.
    let found = find_program("cmd").unwrap();
    assert!(found.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("exe")), "{found:?}");
    let system32 = std::path::Path::new(&std::env::var("SystemRoot").unwrap()).join("System32");
    assert!(on_path(system32.join("cmd").to_str().unwrap()));
    assert!(on_path(system32.join("cmd.exe").to_str().unwrap()));
    assert!(!on_path(r"C:\nonexistent\cmd"));
}

/// A [`FakeMachine`] whose programs are checked for real, with [`on_path`].
#[derive(Clone)]
struct RealPrograms(FakeMachine);

#[async_trait]
impl Machine for RealPrograms {
    async fn locate(&self, harness: AnyHarness, id: &str) -> Option<PathBuf> {
        self.0.locate(harness, id).await
    }

    fn installed(&self, program: &str) -> bool {
        on_path(program)
    }
}

#[rstest]
#[tokio::test]
async fn a_relative_template_program_is_found_from_the_sessions_directory(dirs: Dirs) {
    let machine = RealPrograms(FakeMachine {
        found: HashMap::from([("abc-123".to_owned(), PathBuf::from("/t/abc-123.jsonl"))]),
        missing_programs: vec![],
    });
    let templates = AiSessionResume {
        claude: Some("./bin/wrapper --resume {id}".to_owned()),
        ..AiSessionResume::default()
    };
    let resumer = HarnessResumer::on(templates, machine);
    let session = row(HarnessKind::ClaudeCode, "abc-123", &dirs.elsewhere, false);

    // Not in the session's directory: not installed, whatever the picker's own directory has.
    let err = resumer.plan(&session).await.unwrap_err();
    assert_eq!(err, NotResumable::NotInstalled("./bin/wrapper".to_owned()));

    let wrapper = dirs.elsewhere.join("bin/wrapper");
    std::fs::create_dir_all(wrapper.parent().unwrap()).unwrap();
    std::fs::write(&wrapper, "#!/bin/sh\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let plan = resumer.plan(&session).await.unwrap();
    assert_eq!(plan.program, "./bin/wrapper");
    assert_eq!(plan.cwd.as_deref(), Some(dirs.elsewhere.as_path()));
}

#[rstest]
fn the_program_to_check_is_relative_to_the_plans_directory() {
    let plan = |program: &str, cwd: Option<&str>| ResumePlan {
        program: program.to_owned(),
        args: vec![],
        cwd: cwd.map(PathBuf::from),
        cwd_requirement: CwdRequirement::Preferred,
        native_path: None,
    };
    let joined = Path::new("/w/proj").join("./bin/wrapper").to_string_lossy().into_owned();
    assert_eq!(program_to_check(&plan("./bin/wrapper", Some("/w/proj"))), joined);
    assert_eq!(program_to_check(&plan("./bin/wrapper", None)), "./bin/wrapper");
    assert_eq!(program_to_check(&plan("claude", Some("/w/proj"))), "claude");
    assert_eq!(program_to_check(&plan("/usr/bin/claude", Some("/w/proj"))), "/usr/bin/claude");
}
