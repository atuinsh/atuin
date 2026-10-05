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
///
/// Its copies read as `tip`, an agent has them open as `live` says (none, by default; with
/// `agent_in`, one works there, open for a session in that directory or above it, or in no
/// directory known), and appending records what it would write (from where) and the directory it was told the session
/// works in, unless `refuse` names the error to refuse with.
#[derive(Clone, Default)]
struct FakeMachine {
    found: HashMap<String, PathBuf>,
    written: Arc<Mutex<Vec<RehydrateSession>>>,
    missing_programs: Vec<&'static str>,
    cant_rehydrate: Option<HarnessKind>,
    tip: Option<LocalTip>,
    /// Reading the copy fails, saying this.
    tip_error: Option<&'static str>,
    live: Option<Liveness>,
    live_cwds: Arc<Mutex<Vec<Option<PathBuf>>>>,
    agent_in: Option<PathBuf>,
    refuse: Option<&'static str>,
    appended: Arc<Mutex<Vec<String>>>,
    append_cwds: Arc<Mutex<Vec<Option<PathBuf>>>>,
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

    async fn local_tip(&self, _: AnyHarness, _: &str) -> Result<Option<LocalTip>, SyncError> {
        match self.tip_error {
            Some(why) => Err(SyncError::Unsupported(why)),
            None => Ok(self.tip.clone()),
        }
    }

    async fn is_live(&self, _: AnyHarness, _: &str, cwd: Option<&Path>) -> Liveness {
        self.live_cwds.lock().push(cwd.map(Path::to_path_buf));
        let agent =
            self.agent_in.as_deref().is_some_and(|dir| cwd.is_none_or(|c| dir.starts_with(c)));
        match self.live {
            Some(live) => live,
            None if agent => Liveness::Live { pid: Some(7) },
            None => Liveness::NotLive,
        }
    }

    async fn append(
        &self,
        harness: AnyHarness,
        id: &str,
        base: &LocalTip,
        lines: &[RehydrateMessage],
        options: &AppendOptions<'_>,
    ) -> Result<AppendOutcome, SyncError> {
        self.append_cwds.lock().push(options.cwd.map(Path::to_path_buf));
        if self.is_live(harness, id, options.cwd).await != Liveness::NotLive {
            return Err(SyncError::MaybeLive);
        }
        match self.refuse {
            Some("unsupported") => return Err(SyncError::Unsupported("not a fast-forward")),
            Some("changed") => return Err(SyncError::Changed),
            Some("id taken") => return Err(SyncError::IdTaken("c".to_owned())),
            Some(_) => return Err(SyncError::MaybeLive),
            None => {}
        }
        let ids: Vec<String> = lines.iter().map(|m| m.source_id.clone()).collect();
        let from = base.tip_source_id.clone().unwrap_or_default();
        self.appended.lock().push(format!("{}@{from}", ids.join(",")));
        Ok(AppendOutcome {
            native_path: base.native_path.clone(),
            tip_source_id: ids.last().cloned(),
            appended: ids,
        })
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
            fork_of: None,
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

/// A session with no messages isn't forked either, in any harness, and nothing is written (not
/// even a pi original restored for its fork to name): there'd be nothing to resume, and an
/// opencode fork would have no prompt for its marker. It's refused as its continuation is.
#[rstest]
#[tokio::test]
async fn an_empty_session_is_not_forked(
    dirs: Dirs,
    #[values(HarnessKind::ClaudeCode, HarnessKind::Codex, HarnessKind::Opencode, HarnessKind::Pi)]
    harness: HarnessKind,
) {
    let machine = FakeMachine::default();
    let resumer = resumer(&dirs, &machine);
    let session = row(harness, "abc-123", &dirs.elsewhere, false);
    let err = resumer.fork(&Synced, &session, ForkFrom::default()).await.unwrap_err();
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

/// A fork is a new session of the session's own harness, with its rows (those up to a tip, when
/// given one), linked to the original by both its ids; its status names the harness. Only a
/// harness atuin writes, and that is installed, forks.
#[rstest]
#[tokio::test]
async fn a_fork_is_a_new_session_of_its_own_harness(dirs: Dirs) {
    let machine = FakeMachine::default();
    let resumer = resumer(&dirs, &machine);
    let session = row(HarnessKind::ClaudeCode, "abc-123", &dirs.elsewhere, true);
    assert!(resumer.can_fork(&session));
    let forked = resumer.fork(&Worked, &session, ForkFrom::default()).await.unwrap();

    let written = machine.written.lock().clone();
    let [new] = written.as_slice() else {
        panic!("one session written: {written:?}");
    };
    assert_eq!(new.id, forked.id);
    assert_ne!(new.id, "abc-123");
    let ids: Vec<_> = new.messages.iter().map(|m| m.source_id.as_str()).collect();
    assert_eq!(ids, ["r1", "r2", "r3"], "the rows as they were");
    let of = new.fork_of.clone().unwrap();
    assert_eq!((of.id, of.atuin_id), ("abc-123".to_owned(), Some(session.atuin_id.to_string())));
    assert_eq!(forked.plan.program, "claude");
    assert_eq!(forked.status(), "forked into a new Claude Code session");

    let from = ForkFrom {
        rows: None,
        tip: Some("r2".to_owned()),
    };
    resumer.fork(&Worked, &session, from).await.unwrap();
    assert_eq!(machine.written.lock()[1].messages.len(), 2, "up to the tip");

    let copilot = row(HarnessKind::Copilot, "cp", &dirs.elsewhere, false);
    assert!(!resumer.can_fork(&copilot));
    let missing = FakeMachine {
        missing_programs: vec!["claude"],
        ..FakeMachine::default()
    };
    let without = HarnessResumer::on(context(&dirs), AiSessionResume::default(), missing);
    assert!(!without.can_fork(&session));
}

/// pi names a fork's parent by its file: an original that isn't here is restored first, and the
/// fork names where it was written; one that is here is named where it is.
#[rstest]
#[case::missing(None)]
#[case::here(Some("/pi/sessions/abc-123.jsonl"))]
#[tokio::test]
async fn a_pi_fork_names_the_original_by_its_file(dirs: Dirs, #[case] found: Option<&str>) {
    let mut machine = FakeMachine::default();
    if let Some(found) = found {
        machine.found.insert("abc-123".to_owned(), PathBuf::from(found));
    }
    let resumer = resumer(&dirs, &machine);
    let session = row(HarnessKind::Pi, "abc-123", &dirs.elsewhere, true);
    let forked = resumer.fork(&Worked, &session, ForkFrom::default()).await.unwrap();

    let written: Vec<String> = machine.written.lock().iter().map(|s| s.id.clone()).collect();
    let written: Vec<&str> = written.iter().map(String::as_str).collect();
    let parent = match found {
        None => {
            assert_eq!(written, ["abc-123", forked.id.as_str()], "the original first");
            PathBuf::from("/restored/abc-123.jsonl")
        }
        Some(found) => {
            assert_eq!(written, [forked.id.as_str()]);
            PathBuf::from(found)
        }
    };
    let fork = machine.written.lock().last().cloned().unwrap();
    assert_eq!(fork.fork_of.unwrap().path, Some(parent));
}

/// A Codex fork reads the original's rollout where it is (a reverted thread's names the history
/// it continues, which the fork continues too), and restores nothing when it isn't here.
#[rstest]
#[case::missing(None)]
#[case::here(Some("/codex/sessions/rollout-abc-123.jsonl"))]
#[tokio::test]
async fn a_codex_fork_reads_the_original_where_it_is(dirs: Dirs, #[case] found: Option<&str>) {
    let mut machine = FakeMachine::default();
    if let Some(found) = found {
        machine.found.insert("abc-123".to_owned(), PathBuf::from(found));
    }
    let resumer = resumer(&dirs, &machine);
    let session = row(HarnessKind::Codex, "abc-123", &dirs.elsewhere, true);
    let forked = resumer.fork(&Worked, &session, ForkFrom::default()).await.unwrap();

    let written = machine.written.lock().clone();
    let [fork] = written.as_slice() else {
        panic!("only the fork written: {written:?}");
    };
    assert_eq!(fork.id, forked.id);
    assert_eq!(fork.fork_of.clone().unwrap().path, found.map(PathBuf::from));
}

// --- catching up with sync ----------------------------------------------------------------------

/// What catching the synced Claude Code session up with this machine's copy (read as the
/// machine's `tip`; none: not here) says, and what it appended.
async fn catch_up(dirs: &Dirs, machine: FakeMachine, diverged: bool) -> (String, Vec<String>) {
    catch_up_in(dirs, machine, diverged, &dirs.elsewhere).await
}

/// [`catch_up`], with the session recorded working in `cwd`.
async fn catch_up_in(
    dirs: &Dirs,
    mut machine: FakeMachine,
    diverged: bool,
    cwd: &Path,
) -> (String, Vec<String>) {
    if machine.tip.is_some() {
        machine.found.insert("s".to_owned(), PathBuf::from("/t/s.jsonl"));
    }
    let mut session = row(HarnessKind::ClaudeCode, "s", cwd, false);
    session.handle = fake::synced_handle();
    let source = fake::FakeSource::from_rows(vec![session.clone()])
        .with_synced(&session.handle, fake::synced_rows(diverged));
    let caught = resumer(dirs, &machine).catch_up(&source, &session, None).await.unwrap();
    let said = match caught {
        CatchUp::Ready { plan, status } => {
            format!("{} {}", plan.native_path.unwrap().display(), status.unwrap_or_default())
        }
        CatchUp::Choice(held) => {
            let forks: Vec<&str> = held.branches.iter().map(|b| b.selector.as_str()).collect();
            format!("{} [{}]", held.status(), forks.join(" "))
        }
    };
    (said.trim_end().to_owned(), machine.appended.lock().clone())
}

/// Resuming a session in its own agent catches its copy here up with sync first: restored when
/// it isn't here, resumed as it is when it is at the head, the rows it lacks appended when it is
/// behind; anything else is a choice, with nothing written.
#[rstest]
#[case::not_here(None, None, false, "/restored/s.jsonl", &[])]
#[case::at_the_head(Some((&["a", "b", "c", "d"][..], "d")), None, false, "/t/s.jsonl", &[])]
#[case::behind(
    Some((&["a", "b"][..], "b")),
    None,
    false,
    "/t/s.jsonl caught up: 2 messages from @00000002",
    &["c,d@b"]
)]
#[case::behind_and_live(
    Some((&["a", "b"][..], "b")),
    Some(Liveness::Live { pid: Some(7) }),
    false,
    "Claude Code is running this session here [d]",
    &[]
)]
#[case::behind_and_maybe_live(
    Some((&["a", "b"][..], "b")),
    Some(Liveness::Unknown),
    false,
    "Claude Code is running this session here [d]",
    &[]
)]
#[case::live_at_the_head(
    Some((&["a", "b", "c", "d"][..], "d")),
    Some(Liveness::Unknown),
    false,
    "/t/s.jsonl",
    &[]
)]
#[case::diverged(
    Some((&["a", "b", "c", "d"][..], "d")),
    None,
    true,
    "this copy went another way than this machine's [y d]",
    &[]
)]
#[case::diverged_at_the_newest(
    Some((&["a", "b", "x", "y"][..], "y")),
    None,
    true,
    "/t/s.jsonl",
    &[]
)]
#[case::unsynced(
    Some((&["a", "b", "z"][..], "z")),
    None,
    false,
    "this copy has messages sync hasn't got [d]",
    &[]
)]
#[tokio::test]
async fn a_copy_here_is_caught_up_or_offered_a_choice(
    dirs: Dirs,
    #[case] copy: Option<(&[&str], &str)>,
    #[case] live: Option<Liveness>,
    #[case] diverged: bool,
    #[case] said: &str,
    #[case] appended: &[&str],
) {
    let machine = FakeMachine {
        tip: copy.map(|(known, at)| fake::local_tip(known, Some(at))),
        live,
        ..FakeMachine::default()
    };
    let written = machine.written.clone();
    let caught = catch_up(&dirs, machine, diverged).await;
    let appended = appended.iter().map(|s| (*s).to_owned()).collect();
    assert_eq!(caught, (said.to_owned(), appended));
    assert_eq!(written.lock().len(), usize::from(copy.is_none()), "only a restore writes");
}

/// The agent's copy refusing the rows (it changed, they aren't a clean fast-forward, an id is
/// taken, an agent may have it open after all) leaves the choice, with nothing written.
#[rstest]
#[case::unsupported("unsupported", "couldn't catch up: not a fast-forward [d]")]
#[case::changed(
    "changed",
    "couldn't catch up: the session's transcript changed since it was read [d]"
)]
#[case::id_taken("id taken", "couldn't catch up: c is already taken [d]")]
#[case::maybe_live("maybe live", "Claude Code is running this session here [d]")]
#[tokio::test]
async fn a_refused_append_leaves_the_choice(
    dirs: Dirs,
    #[case] refuse: &'static str,
    #[case] said: &str,
) {
    let machine = FakeMachine {
        tip: Some(fake::local_tip(&["a", "b"], Some("b"))),
        refuse: Some(refuse),
        ..FakeMachine::default()
    };
    assert_eq!(catch_up(&dirs, machine, false).await, (said.to_owned(), Vec::new()));
}

/// The append checks for an agent where the session works, as the plan's check does.
#[rstest]
#[tokio::test]
async fn the_append_looks_for_an_agent_where_the_session_works(dirs: Dirs) {
    let machine = FakeMachine {
        tip: Some(fake::local_tip(&["a", "b"], Some("b"))),
        ..FakeMachine::default()
    };
    let cwds = machine.append_cwds.clone();
    catch_up(&dirs, machine, false).await;
    assert_eq!(*cwds.lock(), vec![Some(dirs.elsewhere.clone())]);
}

#[derive(Debug, Clone, Copy)]
enum Agent {
    InTheMappedDir,
    Unrelated,
}

/// A session recorded on another machine under a path that isn't here works in the same place
/// in this checkout: an agent there may have it open, so nothing is appended; one in an unrelated
/// directory doesn't. One whose directory here isn't known counts any agent as having it open.
#[rstest]
#[case::mapped_agent_there("/home/u/atuin/crates", Agent::InTheMappedDir, false)]
#[case::mapped_agent_elsewhere("/home/u/atuin/crates", Agent::Unrelated, true)]
#[case::unknown_agent_anywhere("/home/u/zsh", Agent::Unrelated, false)]
#[tokio::test]
async fn an_agent_is_looked_for_where_the_session_works_here(
    dirs: Dirs,
    #[case] recorded: &str,
    #[case] agent: Agent,
    #[case] appends: bool,
) {
    let agent_in = match agent {
        Agent::InTheMappedDir => dirs.repo.join("crates").join("atuin"),
        Agent::Unrelated => dirs.elsewhere.clone(),
    };
    let machine = FakeMachine {
        tip: Some(fake::local_tip(&["a", "b"], Some("b"))),
        agent_in: Some(agent_in),
        ..FakeMachine::default()
    };
    let cwds = machine.append_cwds.clone();
    let caught = catch_up_in(&dirs, machine, false, Path::new(recorded)).await;
    let want = if appends {
        ("/t/s.jsonl caught up: 2 messages from @00000002", vec!["c,d@b".to_owned()])
    } else {
        ("Claude Code is running this session here [d]", Vec::new())
    };
    assert_eq!(caught, (want.0.to_owned(), want.1));
    // The append, when it gets that far, looks where the plan did.
    if appends {
        assert_eq!(*cwds.lock(), vec![Some(dirs.repo.join("crates"))]);
    }
}

/// Planning a session whose copy is here says it is to be caught up, and whether an agent here
/// has it open, as the agent's own check says (unknown counting as open).
#[rstest]
#[case::not_live(None, false)]
#[case::live(Some(Liveness::Live { pid: None }), true)]
#[case::unknown(Some(Liveness::Unknown), true)]
#[tokio::test]
async fn a_plan_says_whether_an_agent_here_has_the_session_open(
    dirs: Dirs,
    #[case] live: Option<Liveness>,
    #[case] want: bool,
) {
    let machine = FakeMachine {
        found: HashMap::from([("abc".to_owned(), PathBuf::from("/t/abc.jsonl"))]),
        live,
        ..FakeMachine::default()
    };
    let session = row(HarnessKind::ClaudeCode, "abc", &dirs.elsewhere, false);
    let resume = resumer(&dirs, &machine).plan(&session).await.unwrap();
    assert!(resume.catch_up);
    assert_eq!(resume.live, want);
}

/// The synced Claude Code session (diverged or not), with a title host 2 named last: session
/// metadata, which is no node of the tree.
fn synced_with_a_title(diverged: bool) -> Vec<atuin_client::ai_session::Message> {
    let handle = fake::synced_handle();
    let mut rows = fake::synced_rows(diverged);
    let mut title = fake::synced_row(&handle, ("syn-title", None, 2, 6, false));
    title.role = atuin_common::harnesstools::session::Role::Other("ai-title".to_owned());
    title.content = Vec::new();
    rows.push(title);
    rows
}

/// A session with no copy here is restored along the head named, else, when it diverged, along
/// its newest head (whether `atuin ai resume` catches it up or the picker restores it), with the
/// rows off the tree that go with it; one that went one way is restored from every row synced of
/// it, as before.
#[rstest]
#[case::one_way(false, None, false, &["a", "b", "c", "d", "syn-title"])]
#[case::one_way_by_the_picker(false, None, true, &["a", "b", "c", "d", "syn-title"])]
#[case::one_way_named(false, Some("d"), false, &["a", "b", "c", "d", "syn-title"])]
#[case::diverged(true, None, false, &["a", "b", "x", "y", "syn-title"])]
#[case::diverged_by_the_picker(true, None, true, &["a", "b", "x", "y", "syn-title"])]
#[case::diverged_named(true, Some("d"), false, &["a", "b", "c", "d", "syn-title"])]
#[tokio::test]
async fn a_session_not_here_is_restored_along_one_branch(
    dirs: Dirs,
    #[case] diverged: bool,
    #[case] head: Option<&str>,
    #[case] by_the_picker: bool,
    #[case] want: &[&str],
) {
    let machine = FakeMachine::default();
    let resumer = resumer(&dirs, &machine);
    let mut session = row(HarnessKind::ClaudeCode, "s", &dirs.elsewhere, true);
    session.handle = fake::synced_handle();
    let source = fake::FakeSource::from_rows(vec![session.clone()])
        .with_synced(&session.handle, synced_with_a_title(diverged));
    if by_the_picker {
        let restore = resumer.plan(&session).await.unwrap().restore.unwrap();
        resumer.restore(&source, &session, &restore).await.unwrap();
    } else {
        let head = head.map(|h| SourceId::from(h.to_owned()));
        let caught = resumer.catch_up(&source, &session, head.as_ref()).await.unwrap();
        assert!(matches!(caught, CatchUp::Ready { .. }), "{caught:?}");
    }

    let written = machine.written.lock().clone();
    let [restored] = written.as_slice() else {
        panic!("one session written: {written:?}");
    };
    let ids: Vec<&str> = restored.messages.iter().map(|m| m.source_id.as_str()).collect();
    assert_eq!(ids, want);
}

/// The plan, the catch-up and the append all look for an agent where this machine's copy
/// records working, when it records a directory that is there: not where the session row says
/// (another host's newer directory, maybe). Else where the session works here; else anywhere.
#[rstest]
#[case::where_the_copy_works(true, true)]
#[case::where_the_copy_worked_but_gone(true, false)]
#[case::no_directory_recorded(false, false)]
#[tokio::test]
async fn an_agent_is_looked_for_where_the_copy_works(
    dirs: Dirs,
    #[case] recorded: bool,
    #[case] exists: bool,
) {
    let copy_dir = dirs.repo.join("crates").join("atuin");
    let copy_dir = if exists {
        copy_dir
    } else {
        dirs.repo.join("gone")
    };
    let mut tip = fake::local_tip(&["a", "b"], Some("b"));
    tip.cwd = recorded.then(|| copy_dir.clone());
    let machine = FakeMachine {
        tip: Some(tip),
        ..FakeMachine::default()
    };
    let (live, appended) = (machine.live_cwds.clone(), machine.append_cwds.clone());
    let caught = catch_up(&dirs, machine, false).await;
    assert_eq!(caught.1, ["c,d@b"]);
    let want = if recorded && exists {
        copy_dir
    } else {
        dirs.elsewhere.clone()
    };
    // The plan asks, then the append (the fake asks again as it appends).
    assert_eq!(*live.lock(), vec![Some(want.clone()); 2], "the plan's check");
    assert_eq!(*appended.lock(), vec![Some(want)], "the append's check");

    // An agent where the copy works holds the append, wherever the session row says.
    if recorded && exists {
        let mut tip = fake::local_tip(&["a", "b"], Some("b"));
        tip.cwd = Some(dirs.repo.join("crates").join("atuin"));
        let machine = FakeMachine {
            tip: Some(tip),
            agent_in: Some(dirs.repo.join("crates").join("atuin")),
            ..FakeMachine::default()
        };
        let caught = catch_up(&dirs, machine, false).await;
        assert_eq!(caught, ("Claude Code is running this session here [d]".to_owned(), vec![]));
    }
}

/// A copy that can't be read, or caught up (opencode 2.0's), is never resumed as if it were
/// caught up: the choice says why.
#[rstest]
#[tokio::test]
async fn a_copy_that_cant_be_caught_up_leaves_the_choice(dirs: Dirs) {
    let mut machine = FakeMachine {
        tip_error: Some("opencode 2.0 sessions can't be caught up yet"),
        ..FakeMachine::default()
    };
    machine.found.insert("s".to_owned(), PathBuf::from("/t/s.jsonl"));
    let caught = catch_up(&dirs, machine, false).await;
    assert_eq!(
        caught,
        ("couldn't catch up: opencode 2.0 sessions can't be caught up yet [d]".to_owned(), vec![])
    );
}

/// A copy gone since it was located is restored from sync, as one that isn't here is.
#[rstest]
#[tokio::test]
async fn a_copy_gone_since_it_was_located_is_restored(dirs: Dirs) {
    let mut machine = FakeMachine::default();
    machine.found.insert("s".to_owned(), PathBuf::from("/t/s.jsonl"));
    let written = machine.written.clone();
    let caught = catch_up(&dirs, machine, false).await;
    assert_eq!(caught, ("/restored/s.jsonl".to_owned(), vec![]));
    assert_eq!(written.lock().len(), 1);
}

/// A branch named that is no longer a head is an error, not the newest head instead.
#[rstest]
#[tokio::test]
async fn a_branch_no_longer_a_head_is_not_swapped_for_another(dirs: Dirs) {
    let machine = FakeMachine {
        tip: Some(fake::local_tip(&["a", "b"], Some("b"))),
        found: HashMap::from([("s".to_owned(), PathBuf::from("/t/s.jsonl"))]),
        ..FakeMachine::default()
    };
    let appended = machine.appended.clone();
    let mut session = row(HarnessKind::ClaudeCode, "s", &dirs.elsewhere, false);
    session.handle = fake::synced_handle();
    let source = fake::FakeSource::from_rows(vec![session.clone()])
        .with_synced(&session.handle, fake::synced_rows(false));
    let gone = SourceId::from("b".to_owned());
    let caught = resumer(&dirs, &machine).catch_up(&source, &session, Some(&gone)).await;
    assert!(matches!(caught, Err(NotResumable::CatchUp(_))), "{caught:?}");
    assert!(appended.lock().is_empty());
}
