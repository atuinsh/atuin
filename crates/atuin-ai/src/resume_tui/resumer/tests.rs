use std::collections::HashMap;
use std::sync::Arc;

use atuin_client::ai_session::{HarnessKind, HarnessSession};
use atuin_common::harnesstools::resume::CwdRequirement;
use parking_lot::Mutex;
use rstest::{fixture, rstest};
use tempfile::TempDir;
use time::OffsetDateTime;

use super::*;
use crate::resume_tui::fake;
use crate::resume_tui::source::{SessionFilter, SessionPreview};

/// A machine that touches nothing: transcripts are found where `found` says, restoring records
/// what it would write, and every program is installed unless `installed` says otherwise.
/// Rehydrating fails for the harness `cant_rehydrate` names.
#[derive(Clone, Default)]
struct FakeMachine {
    found: HashMap<String, PathBuf>,
    written: Arc<Mutex<Vec<RehydrateSession>>>,
    missing_programs: Vec<&'static str>,
    cant_rehydrate: Option<HarnessKind>,
}

#[async_trait]
impl Machine for FakeMachine {
    async fn locate(&self, _: AnyHarness, id: &str) -> Option<PathBuf> {
        self.found.get(id).cloned()
    }

    async fn rehydrate(
        &self,
        harness: AnyHarness,
        session: &RehydrateSession,
    ) -> Result<PathBuf, RehydrateError> {
        let path = PathBuf::from(format!("/restored/{}.jsonl", session.id));
        match (harness, session.id.as_str()) {
            (harness, _) if self.cant_rehydrate == Some(HarnessKind::from(&harness)) => {
                Err(RehydrateError::Unsupported(harness_label(HarnessKind::from(&harness))))
            }
            (_, "already-there") => Err(RehydrateError::AlreadyExists(path)),
            _ => {
                self.written.lock().push(session.clone());
                Ok(path)
            }
        }
    }

    fn installed(&self, program: &str) -> bool {
        !self.missing_programs.contains(&program)
    }
}

/// A source whose every session has one message, as the sidecar would give it.
struct Synced;

#[async_trait]
impl SessionSource for Synced {
    async fn search(&self, _: &SessionFilter) -> eyre::Result<Vec<SessionRow>> {
        Ok(Vec::new())
    }

    async fn find_by_id(&self, _: &str) -> eyre::Result<Vec<SessionRow>> {
        Ok(Vec::new())
    }

    async fn preview(&self, _: &HarnessSession) -> eyre::Result<SessionPreview> {
        Ok(SessionPreview::default())
    }

    async fn children(&self, _: &HarnessSession) -> eyre::Result<Vec<SessionRow>> {
        Ok(Vec::new())
    }

    async fn rehydrate(
        &self,
        session: &HarnessSession,
        cwd: &Path,
    ) -> eyre::Result<RehydrateSession> {
        Ok(RehydrateSession {
            id: session.session.to_string(),
            title: None,
            cwd: cwd.to_owned(),
            original_cwd: None,
            git_branch: None,
            model: None,
            started_at: OffsetDateTime::UNIX_EPOCH,
            messages: Vec::new(),
        })
    }
}

/// A scratch filesystem: `here` (the current directory, in a checkout named `atuin` with a
/// `crates/atuin` directory) and `elsewhere`, a directory that exists.
struct Dirs {
    _tmp: TempDir,
    here: PathBuf,
    repo: PathBuf,
    elsewhere: PathBuf,
}

#[fixture]
fn dirs() -> Dirs {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("checkouts").join("atuin");
    let here = repo.join("docs");
    let elsewhere = tmp.path().join("elsewhere");
    for dir in [&here, &repo.join("crates").join("atuin"), &elsewhere] {
        std::fs::create_dir_all(dir).unwrap();
    }
    Dirs {
        _tmp: tmp,
        here,
        repo,
        elsewhere,
    }
}

fn context(dirs: &Dirs) -> ResumeContext {
    ResumeContext {
        cwd: dirs.here.clone(),
        // As the shell's context gives it, with a trailing separator.
        git_root: Some(dirs.repo.join("")),
        host_id: fake::THIS_HOST_ID.to_owned(),
        ..fake::context()
    }
}

fn resumer(dirs: &Dirs, machine: &FakeMachine) -> HarnessResumer {
    HarnessResumer::on(context(dirs), AiSessionResume::default(), machine.clone())
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
    let resume = resumer(&dirs, &machine).plan(&local).await.unwrap();
    assert_eq!(resume.restore, None);
    assert_eq!(resume.plan.args, ["--session", "/t/abc-123.jsonl"]);
    assert_eq!(resume.plan.cwd.as_deref(), Some(dirs.elsewhere.as_path()));
}

/// Another host's session is restored from sync when resumed: planning writes nothing, and the
/// session resumes in the same place in this checkout of its repository.
#[rstest]
#[tokio::test]
async fn a_remote_session_is_planned_as_a_restore(dirs: Dirs) {
    let machine = FakeMachine::default();
    let remote = row(HarnessKind::ClaudeCode, "abc-123", Path::new("/src/atuin/crates"), true);
    let resume = resumer(&dirs, &machine).plan(&remote).await.unwrap();

    let restore = resume.restore.expect("restored from sync");
    assert_eq!(restore.cwd, dirs.repo.join("crates"));
    assert!(restore.note.unwrap().contains("/src/atuin/crates isn't on this machine"));
    assert_eq!(resume.plan.cwd, Some(dirs.repo.join("crates")));
    assert_eq!(resume.plan.args, ["--resume", "abc-123"]);
    assert!(machine.written.lock().is_empty(), "planning never writes");
}

/// This host's session whose transcript is gone is restored too, into its own directory.
#[rstest]
#[tokio::test]
async fn a_local_session_without_its_transcript_is_planned_as_a_restore(dirs: Dirs) {
    let machine = FakeMachine::default();
    let local = row(HarnessKind::ClaudeCode, "abc-123", &dirs.elsewhere, false);
    let resume = resumer(&dirs, &machine).plan(&local).await.unwrap();
    assert_eq!(
        resume.restore,
        Some(Restore {
            cwd: dirs.elsewhere.clone(),
            note: None
        })
    );
}

/// A session restored here before (another host's, or this host's whose transcript was deleted
/// and whose directory is gone) resumes from that transcript, in the directory it was restored
/// into.
#[rstest]
#[tokio::test]
async fn a_session_restored_before_resumes_directly(
    dirs: Dirs,
    #[values(true, false)] remote: bool,
) {
    let machine = FakeMachine {
        found: HashMap::from([("abc-123".to_owned(), PathBuf::from("/t/abc-123.jsonl"))]),
        ..FakeMachine::default()
    };
    let session = row(HarnessKind::ClaudeCode, "abc-123", Path::new("/gone/proj"), remote);
    let resume = resumer(&dirs, &machine).plan(&session).await.unwrap();
    assert_eq!(resume.restore, None);
    assert_eq!(resume.plan.cwd, Some(dirs.here.clone()));
}

/// Restoring writes the session where it resumes and plans resuming that transcript; one
/// already there (restored meanwhile) is resumed as it is.
#[rstest]
#[case::written("abc-123")]
#[case::already_there("already-there")]
#[tokio::test]
async fn restoring_writes_the_transcript_and_plans_resuming_it(dirs: Dirs, #[case] id: &str) {
    let machine = FakeMachine::default();
    let resumer = resumer(&dirs, &machine);
    let remote = row(HarnessKind::Pi, id, Path::new("/gone/proj"), true);
    let restore = resumer.plan(&remote).await.unwrap().restore.unwrap();
    assert_eq!(restore.cwd, dirs.here);

    let plan = resumer.restore(&Synced, &remote, &restore).await.unwrap();
    let path = format!("/restored/{id}.jsonl");
    assert_eq!(plan.args, ["--session", path.as_str()]);
    assert_eq!(plan.cwd.as_deref(), Some(dirs.here.as_path()));
    let written: Vec<_> = machine.written.lock().iter().map(|s| s.cwd.clone()).collect();
    if id == "abc-123" {
        assert_eq!(written, std::slice::from_ref(&dirs.here));
    } else {
        assert!(written.is_empty());
    }
}

/// A template using `{path}` doesn't stop a session from being restored: the plan shows where
/// the transcript will go, and restoring plans it with the path it was written to.
#[rstest]
#[tokio::test]
async fn a_path_template_plans_a_restore_and_resumes_the_restored_transcript(dirs: Dirs) {
    let machine = FakeMachine::default();
    let templates = AiSessionResume {
        pi: Some("my-pi --session {path}".to_owned()),
        ..AiSessionResume::default()
    };
    let resumer = HarnessResumer::on(context(&dirs), templates, machine.clone());
    let remote = row(HarnessKind::Pi, "abc-123", Path::new("/gone/proj"), true);

    let resume = resumer.plan(&remote).await.unwrap();
    assert_eq!(resume.plan.program, "my-pi");
    assert_eq!(resume.plan.args, ["--session", RESTORED_PATH_PLACEHOLDER]);
    let restore = resume.restore.expect("restored from sync");

    let plan = resumer.restore(&Synced, &remote, &restore).await.unwrap();
    assert_eq!(plan.program, "my-pi");
    assert_eq!(plan.args, ["--session", "/restored/abc-123.jsonl"]);
    assert_eq!(machine.written.lock().len(), 1);
}

#[rstest]
#[tokio::test]
async fn a_harness_that_cant_rehydrate_says_so(dirs: Dirs) {
    let machine = FakeMachine {
        cant_rehydrate: Some(HarnessKind::Opencode),
        ..FakeMachine::default()
    };
    let resumer = resumer(&dirs, &machine);
    let remote = row(HarnessKind::Opencode, "ses_0199aaaa", Path::new("/gone"), true);
    let restore = resumer.plan(&remote).await.unwrap().restore.unwrap();
    let err = resumer.restore(&Synced, &remote, &restore).await.unwrap_err();
    assert_eq!(
        err.to_string(),
        "restoring it from sync failed: opencode sessions can't be rehydrated"
    );
}

#[rstest]
#[tokio::test]
async fn a_harness_not_installed_here_says_so(dirs: Dirs) {
    let machine = FakeMachine {
        missing_programs: vec!["claude"],
        ..FakeMachine::default()
    };
    let remote = row(HarnessKind::ClaudeCode, "abc-123", &dirs.elsewhere, true);
    let err = resumer(&dirs, &machine).plan(&remote).await.unwrap_err();
    assert_eq!(err, NotResumable::NotInstalled("claude".to_owned()));
    assert_eq!(err.to_string(), "`claude` isn't installed here (not found on PATH)");
}

#[rstest]
#[tokio::test]
async fn unsupported_sessions_and_subagents_are_not_resumable(dirs: Dirs) {
    let machine = FakeMachine::default();
    let resumer = resumer(&dirs, &machine);
    let copilot = row(HarnessKind::Copilot, "cp-1", &dirs.here, false);
    assert_eq!(resumer.plan(&copilot).await.unwrap_err(), NotResumable::Unsupported("Copilot"));

    // A Claude Code subagent is refused by the harness before anything is looked up.
    let subagent = row(HarnessKind::ClaudeCode, "agent-a1b2", &dirs.here, true);
    assert!(matches!(
        resumer.plan(&subagent).await.unwrap_err(),
        NotResumable::Harness(ResumeError::NotResumable(_))
    ));
}

#[derive(Debug, Clone, Copy)]
enum Expect {
    Original,
    RepoSubdir,
    RepoNamesake,
    RepoRoot,
    Here,
}

/// A session resumes where it ran when that exists here; else in the same place in this
/// checkout of its repository (or the checkout's root); else in the current directory.
#[rstest]
#[case::exists(Some("ELSEWHERE"), Expect::Original, false)]
#[case::same_repo_subdir(Some("/home/u/atuin/crates"), Expect::RepoSubdir, true)]
#[case::same_repo_gone_subdir(Some("/home/u/atuin/gone"), Expect::RepoRoot, true)]
#[case::same_repo_root(Some("/home/u/atuin"), Expect::RepoRoot, true)]
#[case::same_repo_dir_named_like_it(Some("/home/u/atuin/crates/atuin"), Expect::RepoNamesake, true)]
#[case::other_repo(Some("/home/u/zsh"), Expect::Here, true)]
#[case::unknown(None, Expect::Here, true)]
fn resumes_where_it_ran_or_nearest_here(
    dirs: Dirs,
    #[case] original: Option<&str>,
    #[case] expect: Expect,
    #[case] noted: bool,
) {
    let original = original.map(|o| {
        if o == "ELSEWHERE" {
            dirs.elsewhere.clone()
        } else {
            PathBuf::from(o)
        }
    });
    let restore = resolve_cwd(original.as_deref(), &context(&dirs));
    let expected = match expect {
        Expect::Original => dirs.elsewhere,
        Expect::RepoSubdir => dirs.repo.join("crates"),
        Expect::RepoNamesake => dirs.repo.join("crates").join("atuin"),
        Expect::RepoRoot => dirs.repo,
        Expect::Here => dirs.here,
    };
    assert_eq!(restore.cwd, expected);
    assert_eq!(restore.note.is_some(), noted, "{restore:?}");
}

#[derive(Debug, Clone, Copy)]
enum Cwd {
    InOuter,
    InInner,
    Elsewhere,
}

/// A session that ran in a directory naming the checkout twice, both of whose places exist in
/// this checkout: the one holding the current directory, else the first from the left.
#[rstest]
#[case::in_the_first(Cwd::InOuter, "other/atuin/crates")]
#[case::in_the_second(Cwd::InInner, "crates")]
#[case::in_neither(Cwd::Elsewhere, "other/atuin/crates")]
fn a_checkout_named_twice_resumes_where_the_current_directory_is(
    dirs: Dirs,
    #[case] cwd: Cwd,
    #[case] expected: &str,
) {
    let outer = dirs.repo.join("other").join("atuin").join("crates");
    let inner = dirs.repo.join("crates");
    std::fs::create_dir_all(outer.join("deeper")).unwrap();
    std::fs::create_dir_all(inner.join("deeper")).unwrap();
    let context = ResumeContext {
        cwd: match cwd {
            Cwd::InOuter => outer.join("deeper"),
            Cwd::InInner => inner.join("deeper"),
            Cwd::Elsewhere => dirs.here.clone(),
        },
        ..context(&dirs)
    };
    let original = Path::new("/work/atuin/other/atuin/crates");
    let restore = resolve_cwd(Some(original), &context);
    let expected: PathBuf = expected.split('/').fold(dirs.repo, |p, c| p.join(c));
    assert_eq!(restore.cwd, expected);
}

/// A source whose sessions call a tool and reason, as captured.
struct Worked;

#[async_trait]
impl SessionSource for Worked {
    async fn search(&self, _: &SessionFilter) -> eyre::Result<Vec<SessionRow>> {
        Ok(Vec::new())
    }

    async fn find_by_id(&self, _: &str) -> eyre::Result<Vec<SessionRow>> {
        Ok(Vec::new())
    }

    async fn preview(&self, _: &HarnessSession) -> eyre::Result<SessionPreview> {
        Ok(SessionPreview::default())
    }

    async fn children(&self, _: &HarnessSession) -> eyre::Result<Vec<SessionRow>> {
        Ok(Vec::new())
    }

    async fn rehydrate(
        &self,
        session: &HarnessSession,
        cwd: &Path,
    ) -> eyre::Result<RehydrateSession> {
        use atuin_common::harnesstools::rehydrate::RehydrateMessage;
        use atuin_common::harnesstools::session::{Content, Role, ToolUse};
        let row = |n: i64, role, content| RehydrateMessage {
            source_id: format!("r{n}"),
            parent_source_id: None,
            timestamp: OffsetDateTime::from_unix_timestamp(n).unwrap(),
            role,
            content,
            model: None,
            usage: None,
            stop_reason: None,
            turn_id: None,
            cwd: None,
            git_branch: None,
        };
        let mut synced = Synced.rehydrate(session, cwd).await?;
        synced.messages = vec![
            row(1, Role::User, vec![Content::Text("fix it".into())]),
            row(2, Role::Assistant, vec![
                Content::ReasoningSummary { tokens: None },
                Content::ToolUse(ToolUse {
                    id: "t1".to_owned().into(),
                    name: "Bash".to_owned(),
                    input: serde_json::json!({"command": "cargo test"}),
                }),
            ]),
            row(3, Role::Assistant, vec![Content::Text("fixed".into())]),
        ];
        Ok(synced)
    }
}

/// Continuing writes a new session of the target, never the original's id, where the session
/// ran, and plans resuming it with the target's own command; the status says what was
/// flattened.
#[rstest]
#[tokio::test]
async fn continuing_writes_a_new_session_of_the_target(dirs: Dirs) {
    let machine = FakeMachine::default();
    let resumer = resumer(&dirs, &machine);
    let session = row(HarnessKind::ClaudeCode, "abc-123", &dirs.elsewhere, true);
    let continued = resumer.continue_in(&Worked, &session, HarnessKind::Pi).await.unwrap();

    let written = machine.written.lock().clone();
    let [new] = written.as_slice() else {
        panic!("one session written: {written:?}");
    };
    assert_ne!(new.id, "abc-123");
    // Its marker (pi's: the first prompt's first block) names the original by both ids.
    let marker = format!("{:?}", new.messages[0].content[0]);
    assert!(marker.contains("session abc-123"), "{marker}");
    assert!(marker.contains(&format!("(atuin id {})", session.atuin_id)), "{marker}");
    assert_eq!(new.cwd, dirs.elsewhere);
    assert_eq!(continued.target, HarnessKind::Pi);
    let path = format!("/restored/{}.jsonl", new.id);
    assert_eq!(continued.plan.program, "pi");
    assert_eq!(continued.plan.args, ["--session", path.as_str()]);
    assert_eq!(continued.plan.cwd.as_deref(), Some(dirs.elsewhere.as_path()));
    assert_eq!(
        continued.status(),
        "continuing in Pi: 1 tool call becomes a note, reasoning dropped"
    );
}

/// A session with no messages isn't continued anywhere, and nothing is written: there is
/// nothing to carry over (and in pi, no prompt for the marker linking it to ride on).
#[rstest]
#[tokio::test]
async fn an_empty_session_is_not_continued(
    dirs: Dirs,
    #[values(HarnessKind::Codex, HarnessKind::Opencode, HarnessKind::Pi)] target: HarnessKind,
) {
    let machine = FakeMachine::default();
    let resumer = resumer(&dirs, &machine);
    let session = row(HarnessKind::ClaudeCode, "abc-123", &dirs.elsewhere, true);
    let err = resumer.continue_in(&Synced, &session, target).await.unwrap_err();
    assert_eq!(err, NotResumable::Empty(continuation::NothingToContinue));
    assert_eq!(err.to_string(), "nothing to continue: the session has no messages");
    assert!(machine.written.lock().is_empty());
}

/// A target's template using `{path}` still offers it, and the continuation resumes with the
/// path its transcript was written to.
#[rstest]
#[tokio::test]
async fn a_path_template_target_is_offered_and_resumes_the_written_session(dirs: Dirs) {
    let machine = FakeMachine::default();
    let templates = AiSessionResume {
        pi: Some("pi --session {path}".to_owned()),
        ..AiSessionResume::default()
    };
    let resumer = HarnessResumer::on(context(&dirs), templates, machine.clone());
    let session = row(HarnessKind::ClaudeCode, "abc-123", &dirs.elsewhere, true);
    assert!(resumer.continue_targets(&session).contains(&HarnessKind::Pi));

    let continued = resumer.continue_in(&Worked, &session, HarnessKind::Pi).await.unwrap();
    let written = machine.written.lock().clone();
    let path = format!("/restored/{}.jsonl", written[0].id);
    assert_eq!(continued.plan.args, ["--session", path.as_str()]);
}

/// Only the other harnesses that are installed are offered; continuing in one that isn't, or in
/// the session's own, writes nothing.
#[rstest]
#[tokio::test]
async fn only_other_installed_harnesses_are_offered(dirs: Dirs) {
    let machine = FakeMachine {
        missing_programs: vec!["opencode"],
        ..FakeMachine::default()
    };
    let resumer = resumer(&dirs, &machine);
    let session = row(HarnessKind::ClaudeCode, "abc-123", &dirs.elsewhere, false);
    assert_eq!(resumer.continue_targets(&session), [HarnessKind::Codex, HarnessKind::Pi]);

    let err = resumer.continue_in(&Worked, &session, HarnessKind::Opencode).await.unwrap_err();
    assert_eq!(err, NotResumable::NotInstalled("opencode".to_owned()));
    let err = resumer.continue_in(&Worked, &session, HarnessKind::ClaudeCode).await.unwrap_err();
    assert!(matches!(err, NotResumable::Continue("Claude Code", _)), "{err}");
    assert!(machine.written.lock().is_empty());

    let copilot = row(HarnessKind::Copilot, "cp-1", &dirs.here, false);
    assert!(resumer.continue_targets(&copilot).is_empty());
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

    async fn rehydrate(
        &self,
        harness: AnyHarness,
        session: &RehydrateSession,
    ) -> Result<PathBuf, RehydrateError> {
        self.0.rehydrate(harness, session).await
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
        ..FakeMachine::default()
    });
    let templates = AiSessionResume {
        claude: Some("./bin/wrapper --resume {id}".to_owned()),
        ..AiSessionResume::default()
    };
    let resumer = HarnessResumer::on(context(&dirs), templates, machine);
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
    let plan = resumer.plan(&session).await.unwrap().plan;
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
