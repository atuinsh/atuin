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
#[derive(Clone, Default)]
struct FakeMachine {
    found: HashMap<String, PathBuf>,
    written: Arc<Mutex<Vec<RehydrateSession>>>,
    missing_programs: Vec<&'static str>,
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
            (AnyHarness::Codex(_), _) => Err(RehydrateError::Unsupported("Codex")),
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

    async fn children(&self, _: &HarnessSession, _: bool) -> eyre::Result<Vec<SessionRow>> {
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
/// `crates` directory) and `elsewhere`, a directory that exists.
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
    for dir in [&here, &repo.join("crates"), &elsewhere] {
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
        git_root: Some(dirs.repo.clone()),
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
        row.hostname = "buildbox".to_owned();
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

/// A remote session restored here before resumes from that transcript, in the directory it was
/// restored into.
#[rstest]
#[tokio::test]
async fn a_remote_session_restored_before_resumes_directly(dirs: Dirs) {
    let machine = FakeMachine {
        found: HashMap::from([("abc-123".to_owned(), PathBuf::from("/t/abc-123.jsonl"))]),
        ..FakeMachine::default()
    };
    let remote = row(HarnessKind::ClaudeCode, "abc-123", Path::new("/gone/proj"), true);
    let resume = resumer(&dirs, &machine).plan(&remote).await.unwrap();
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
        assert_eq!(written, [dirs.here.clone()]);
    } else {
        assert!(written.is_empty());
    }
}

#[rstest]
#[tokio::test]
async fn a_harness_that_cant_rehydrate_says_so(dirs: Dirs) {
    let machine = FakeMachine::default();
    let resumer = resumer(&dirs, &machine);
    let remote = row(HarnessKind::Codex, "0199aaaa", Path::new("/gone"), true);
    let restore = resumer.plan(&remote).await.unwrap().restore.unwrap();
    let err = resumer.restore(&Synced, &remote, &restore).await.unwrap_err();
    assert_eq!(
        err.to_string(),
        "restoring it from sync failed: Codex sessions can't be rehydrated"
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
        Expect::Original => dirs.elsewhere.clone(),
        Expect::RepoSubdir => dirs.repo.join("crates"),
        Expect::RepoRoot => dirs.repo.clone(),
        Expect::Here => dirs.here.clone(),
    };
    assert_eq!(restore.cwd, expected);
    assert_eq!(restore.note.is_some(), noted, "{restore:?}");
}

#[rstest]
fn finds_programs_on_path() {
    assert!(on_path("sh"));
    assert!(!on_path("definitely-not-a-program-atuin"));
    assert!(on_path("/bin/sh"));
    assert!(!on_path("/nonexistent/sh"));
}
