use std::collections::{HashMap, HashSet};
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
///
/// Its harnesses sync the fake way: the copy here is `tip` (none without), except Codex's,
/// which can't be caught up; a harness has it open as `live` says; and appending records what
/// it would write, or fails as `append_fails` says.
#[derive(Clone)]
struct FakeMachine {
    found: HashMap<String, PathBuf>,
    written: Arc<Mutex<Vec<RehydrateSession>>>,
    missing_programs: Vec<&'static str>,
    tip: Option<LocalTip>,
    live: Liveness,
    append_fails: Option<fn() -> SyncError>,
    appended: Arc<Mutex<Vec<Appended>>>,
}

impl Default for FakeMachine {
    fn default() -> Self {
        Self {
            found: HashMap::new(),
            written: Arc::default(),
            missing_programs: Vec::new(),
            tip: None,
            live: Liveness::NotLive,
            append_fails: None,
            appended: Arc::default(),
        }
    }
}

/// What a [`FakeMachine`] was asked to append.
#[derive(Clone, Debug)]
struct Appended {
    lines: Vec<String>,
    head: Option<String>,
    make_tip: bool,
    taken: HashSet<String>,
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

    async fn local_tip(&self, harness: AnyHarness, _: &str) -> Result<Option<LocalTip>, SyncError> {
        match harness {
            AnyHarness::Codex(_) => {
                Err(SyncError::Unsupported("Codex sessions can't be caught up yet"))
            }
            _ => Ok(self.tip.clone()),
        }
    }

    async fn is_live(&self, _: AnyHarness, _: &str, _: Option<&Path>) -> Liveness {
        self.live
    }

    async fn append(
        &self,
        _: AnyHarness,
        _: &str,
        base: &LocalTip,
        lines: &[RehydrateMessage],
        options: &AppendOptions<'_>,
    ) -> Result<AppendOutcome, SyncError> {
        if let Some(fail) = self.append_fails {
            return Err(fail());
        }
        let lines: Vec<String> = lines.iter().map(|l| l.source_id.clone()).collect();
        self.appended.lock().push(Appended {
            lines: lines.clone(),
            head: options.head.map(str::to_owned),
            make_tip: options.make_tip,
            taken: options.taken_ids.cloned().unwrap_or_default(),
        });
        Ok(AppendOutcome {
            native_path: base.native_path.clone(),
            tip_source_id: lines.last().cloned().or(options.head.map(str::to_owned)),
            appended: lines,
            marked_tip: true,
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
        assert_eq!(written, std::slice::from_ref(&dirs.here));
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
        Expect::Original => dirs.elsewhere,
        Expect::RepoSubdir => dirs.repo.join("crates"),
        Expect::RepoRoot => dirs.repo,
        Expect::Here => dirs.here,
    };
    assert_eq!(restore.cwd, expected);
    assert_eq!(restore.note.is_some(), noted, "{restore:?}");
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
            seq: None,
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
    assert_eq!(new.cwd, dirs.elsewhere);
    assert_eq!(continued.target, HarnessKind::Pi);
    let path = format!("/restored/{}.jsonl", new.id);
    assert_eq!(continued.plan.program, "pi");
    assert_eq!(continued.plan.args, ["--session", path.as_str()]);
    assert_eq!(continued.plan.cwd.as_deref(), Some(dirs.elsewhere.as_path()));
    assert_eq!(
        continued.status(),
        "continuing in Pi: 1 tool call flattened to a note, reasoning dropped"
    );
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
    assert!(on_path("sh"));
    assert!(!on_path("definitely-not-a-program-atuin"));
    assert!(on_path("/bin/sh"));
    assert!(!on_path("/nonexistent/sh"));
}

// --- catching up with sync -----------------------------------------------------------------------

mod sync {
    use atuin_client::ai_session::{Head, SessionHeads, SourceId};
    use atuin_common::harnesstools::sync::Stamp;
    use time::Duration;

    use super::*;
    use crate::resume_tui::catchup::{Caught, Kept};
    use crate::resume_tui::fake::{FakeBranches, FakeSource, OTHER_HOST_ID, THIS_HOST_ID};

    const ID: &str = "7aaabc31-1631-4756-8810-d033deb08da5";
    const NATIVE: &str = "/t/7aaabc31.jsonl";

    /// A session that went one way: 12 rows, the last six buildbox's.
    fn one_head() -> FakeBranches {
        let mut rows = fake::chain("s", 6, None, 60);
        rows.extend(fake::chain("m", 6, Some("s6"), 10));
        FakeBranches {
            heads: SessionHeads {
                heads: vec![Head {
                    source_id: SourceId::from("m6".to_owned()),
                    host: Some(fake::host(OTHER_HOST_ID)),
                    last_at: fake::now() - Duration::minutes(10),
                    rows: 12,
                }],
                branch_point: None,
                diverged: false,
            },
            rows,
        }
    }

    /// This machine's copy, holding `known` and continuing from `tip`.
    fn tip(known: &[&str], tip: Option<&str>) -> LocalTip {
        LocalTip {
            native_path: PathBuf::from(NATIVE),
            known_source_ids: known.iter().map(|s| (*s).to_owned()).collect(),
            tip_source_id: tip.map(str::to_owned),
            stamp: Stamp::of(b"copy"),
        }
    }

    fn ids(prefix: &str, n: usize) -> Vec<String> {
        (1..=n).map(|i| format!("{prefix}{i}")).collect()
    }

    /// The session, of `harness`, with its copy here (found where `NATIVE` says) when `local`,
    /// and a machine holding `copy`.
    fn setup(
        dirs: &Dirs,
        harness: HarnessKind,
        branches: FakeBranches,
        copy: Option<LocalTip>,
    ) -> (FakeSource, SessionRow, FakeMachine) {
        let local = copy.is_some();
        let mut session = row(harness, ID, &dirs.elsewhere, false);
        session.host_id = THIS_HOST_ID.to_owned();
        session.heads.clone_from(&branches.heads.heads);
        session.diverged = branches.heads.diverged;
        let source = FakeSource::only(session.clone(), branches);
        let machine = FakeMachine {
            found: if local {
                HashMap::from([(ID.to_owned(), PathBuf::from(NATIVE))])
            } else {
                HashMap::new()
            },
            tip: copy,
            ..FakeMachine::default()
        };
        (source, session, machine)
    }

    fn head(id: &str) -> SourceId {
        SourceId::from(id.to_owned())
    }

    /// Nothing here: written out from sync, the whole session when it went one way.
    #[rstest]
    #[tokio::test]
    async fn no_copy_here_is_restored(dirs: Dirs) {
        let (source, session, machine) = setup(&dirs, HarnessKind::ClaudeCode, one_head(), None);
        let synced = resumer(&dirs, &machine).sync(&source, &session, None).await.unwrap();
        assert_eq!(synced.caught, Caught::Restored {
            rows: 12,
            note: None
        });
        let written = machine.written.lock().clone();
        assert_eq!(written.len(), 1);
        assert_eq!(written[0].id, ID);
        assert_eq!(synced.plan.args, ["--resume", ID]);
        assert!(machine.appended.lock().is_empty());
    }

    /// Nothing here, several branches: written out along the one picked, only.
    #[rstest]
    #[case::latest(None, "b", 40)]
    #[case::picked(Some("h24"), "h", 24)]
    #[tokio::test]
    async fn no_copy_here_is_restored_along_the_branch(
        dirs: Dirs,
        #[case] picked: Option<&str>,
        #[case] prefix: &str,
        #[case] rows: usize,
    ) {
        let branches = fake::diverged_branches();
        let (source, session, machine) = setup(&dirs, HarnessKind::ClaudeCode, branches, None);
        let picked = picked.map(head);
        let synced =
            resumer(&dirs, &machine).sync(&source, &session, picked.as_ref()).await.unwrap();
        assert_eq!(synced.caught, Caught::Restored {
            rows: 6 + rows,
            note: None
        });
        let written = machine.written.lock().clone();
        let lines: Vec<String> = written[0].messages.iter().map(|m| m.source_id.clone()).collect();
        let mut expected = ids("s", 6);
        expected.extend(ids(prefix, rows));
        assert_eq!(lines, expected);
        assert_eq!(synced.others.len(), 1, "the other branch is named");
    }

    /// A copy at the head resumes as it is; one behind on the branch is fast-forwarded, the rows
    /// it lacks appended and the last made the tip, taking no id the session holds elsewhere.
    #[rstest]
    #[tokio::test]
    async fn a_copy_behind_is_fast_forwarded(dirs: Dirs) {
        let copy = tip(&["s1", "s2", "s3", "s4", "s5", "s6"], Some("s6"));
        let (source, session, machine) =
            setup(&dirs, HarnessKind::ClaudeCode, one_head(), Some(copy));
        let synced = resumer(&dirs, &machine).sync(&source, &session, None).await.unwrap();
        assert_eq!(synced.caught, Caught::FastForwarded { rows: 6 });
        assert_eq!(synced.plan.args, ["--resume", ID]);
        let appended = machine.appended.lock().clone();
        let [appended] = appended.as_slice() else {
            panic!("{appended:?}");
        };
        assert_eq!(appended.lines, ids("m", 6));
        assert!(appended.make_tip && appended.head.is_none());
        assert!(appended.taken.contains("s1") && !appended.taken.contains("m1"));
        assert!(machine.written.lock().is_empty(), "nothing written afresh");

        let names = |h: &Head| {
            if h.host == Some(fake::host(OTHER_HOST_ID)) {
                "@buildbox".to_owned()
            } else {
                "this machine".to_owned()
            }
        };
        assert_eq!(
            synced.status(HarnessKind::ClaudeCode, &names).unwrap(),
            "caught up 6 messages from @buildbox"
        );
    }

    #[rstest]
    #[tokio::test]
    async fn a_copy_at_the_head_resumes_as_it_is(dirs: Dirs) {
        let known: Vec<String> = ids("s", 6).into_iter().chain(ids("m", 6)).collect();
        let known: Vec<&str> = known.iter().map(String::as_str).collect();
        let (source, session, machine) =
            setup(&dirs, HarnessKind::ClaudeCode, one_head(), Some(tip(&known, Some("m6"))));
        let synced = resumer(&dirs, &machine).sync(&source, &session, None).await.unwrap();
        assert_eq!(synced.caught, Caught::UpToDate);
        assert!(machine.appended.lock().is_empty());
    }

    /// A harness with the session open here (or maybe open): nothing is written, and the copy
    /// resumes as it is once the user says so. At the head, it just resumes.
    #[rstest]
    #[case::live(Liveness::Live { pid: Some(42) }, Some(42))]
    #[case::unknown(Liveness::Unknown, None)]
    #[tokio::test]
    async fn never_under_a_live_harness(
        dirs: Dirs,
        #[case] live: Liveness,
        #[case] pid: Option<u32>,
    ) {
        let copy = tip(&["s1", "s2", "s3"], Some("s3"));
        let (source, session, mut machine) =
            setup(&dirs, HarnessKind::ClaudeCode, one_head(), Some(copy));
        machine.live = live;
        let synced = resumer(&dirs, &machine).sync(&source, &session, None).await.unwrap();
        assert_eq!(synced.caught, Caught::Kept(Kept::Live { pid }));
        assert!(synced.holds());
        assert_eq!(synced.plan.args, ["--resume", ID]);
        assert!(machine.appended.lock().is_empty());
        let status = synced.status(HarnessKind::ClaudeCode, &|_| String::new()).unwrap();
        assert_eq!(status, "Claude Code is running this session here — close it to catch up");

        // Refused by the harness itself (it opened meanwhile): the same.
        machine.live = Liveness::NotLive;
        machine.append_fails = Some(|| SyncError::Live(Some(42)));
        let synced = resumer(&dirs, &machine).sync(&source, &session, None).await.unwrap();
        assert_eq!(synced.caught, Caught::Kept(Kept::Live { pid: Some(42) }));

        let known: Vec<String> = ids("s", 6).into_iter().chain(ids("m", 6)).collect();
        let known: Vec<&str> = known.iter().map(String::as_str).collect();
        machine.tip = Some(tip(&known, Some("m6")));
        machine.live = live;
        let synced = resumer(&dirs, &machine).sync(&source, &session, None).await.unwrap();
        assert_eq!(synced.caught, Caught::UpToDate, "nothing to write: resumes");
    }

    /// The real case: this machine's copy stopped on its own branch (h, 24 rows past the shared
    /// six), buildbox went on (b, 40): picking buildbox's appends it beside this machine's, in
    /// the same session, and makes it the tip; picking this machine's resumes as it is.
    #[rstest]
    #[tokio::test]
    async fn a_diverged_copy_gets_the_branch_picked_beside_its_own(dirs: Dirs) {
        let mine: Vec<String> = ids("s", 6).into_iter().chain(ids("h", 24)).collect();
        let mine: Vec<&str> = mine.iter().map(String::as_str).collect();
        let branches = fake::diverged_branches();
        let copy = tip(&mine, Some("h24"));
        let (source, session, machine) =
            setup(&dirs, HarnessKind::ClaudeCode, branches, Some(copy));
        let resumer = resumer(&dirs, &machine);

        let synced = resumer.sync(&source, &session, Some(&head("b40"))).await.unwrap();
        assert_eq!(synced.caught, Caught::Switched { rows: 40 });
        assert_eq!(synced.plan.args, ["--resume", ID], "the same session");
        let appended = machine.appended.lock().clone();
        assert_eq!(appended[0].lines, ids("b", 40));
        assert!(appended[0].make_tip);
        assert!(appended[0].taken.contains("h24"));

        machine.appended.lock().clear();
        let synced = resumer.sync(&source, &session, Some(&head("h24"))).await.unwrap();
        assert_eq!(synced.caught, Caught::UpToDate);
        assert!(machine.appended.lock().is_empty());
    }

    /// A copy that already holds the branch picked, but continues from another, is switched to
    /// it: nothing appended, the head made the tip.
    #[rstest]
    #[tokio::test]
    async fn a_branch_held_here_is_made_the_tip(dirs: Dirs) {
        let all: Vec<String> =
            ids("s", 6).into_iter().chain(ids("h", 24)).chain(ids("b", 40)).collect();
        let all: Vec<&str> = all.iter().map(String::as_str).collect();
        let copy = tip(&all, Some("h24"));
        let (source, session, machine) =
            setup(&dirs, HarnessKind::ClaudeCode, fake::diverged_branches(), Some(copy));
        let synced =
            resumer(&dirs, &machine).sync(&source, &session, Some(&head("b40"))).await.unwrap();
        assert_eq!(synced.caught, Caught::Switched { rows: 0 });
        let appended = machine.appended.lock().clone();
        assert!(appended[0].lines.is_empty());
        assert_eq!(appended[0].head.as_deref(), Some("b40"));
    }

    /// opencode keeps a session as one line and refuses a branch in place: the branch is
    /// continued as a new session linked to this one, saying so.
    #[rstest]
    #[tokio::test]
    async fn a_harness_that_cant_take_the_branch_forks_it(dirs: Dirs) {
        let mine: Vec<String> = ids("s", 6).into_iter().chain(ids("h", 24)).collect();
        let mine: Vec<&str> = mine.iter().map(String::as_str).collect();
        let copy = tip(&mine, Some("h24"));
        let (source, session, mut machine) =
            setup(&dirs, HarnessKind::Opencode, fake::diverged_branches(), Some(copy));
        machine.append_fails = Some(|| {
            SyncError::Unsupported(
                "opencode keeps a session as one line: these messages would land before its last",
            )
        });
        let synced =
            resumer(&dirs, &machine).sync(&source, &session, Some(&head("b40"))).await.unwrap();
        let Caught::Forked { why, .. } = &synced.caught else {
            panic!("{:?}", synced.caught);
        };
        assert!(why.starts_with("opencode keeps a session as one line"), "{why}");
        let written = machine.written.lock().clone();
        let [new] = written.as_slice() else {
            panic!("{written:?}");
        };
        assert_ne!(new.id, ID, "a new session");
        assert!(synced.plan.args.contains(&new.id), "{:?}", synced.plan.args);
        let status = synced.status(HarnessKind::Opencode, &|_| "@buildbox".to_owned()).unwrap();
        assert!(
            status.starts_with(
                "opencode can't take @buildbox's branch in place (opencode keeps a session"
            ),
            "{status}"
        );
        assert!(status.contains("continuing it as a new linked session"), "{status}");
    }

    /// Codex can't be caught up yet: its copy resumes as it is, another host's branch of a
    /// diverged session is continued as a session of its own, and without a copy it is
    /// restored along the branch.
    #[rstest]
    #[tokio::test]
    async fn a_harness_that_cant_catch_up_does_as_before(dirs: Dirs) {
        let copy = tip(&[], None);
        let (source, session, machine) =
            setup(&dirs, HarnessKind::Codex, one_head(), Some(copy.clone()));
        let synced = resumer(&dirs, &machine).sync(&source, &session, None).await.unwrap();
        assert_eq!(synced.caught, Caught::UpToDate);
        assert!(machine.written.lock().is_empty() && machine.appended.lock().is_empty());

        let (source, session, machine) =
            setup(&dirs, HarnessKind::Codex, fake::diverged_branches(), Some(copy.clone()));
        let resumer_here = resumer(&dirs, &machine);
        let synced = resumer_here.sync(&source, &session, Some(&head("h24"))).await.unwrap();
        assert_eq!(synced.caught, Caught::UpToDate, "this machine's branch is the copy here");
        // Codex can't write a fork out here (the fake machine's Codex can't rehydrate): it
        // says so.
        let err = resumer_here.sync(&source, &session, Some(&head("b40"))).await.unwrap_err();
        assert!(matches!(err, NotResumable::CatchUp(_)), "{err}");

        let (source, session, machine) =
            setup(&dirs, HarnessKind::Codex, fake::diverged_branches(), None);
        let err = resumer(&dirs, &machine).sync(&source, &session, None).await.unwrap_err();
        assert_eq!(
            err.to_string(),
            "restoring it from sync failed: Codex sessions can't be rehydrated"
        );
    }

    /// A copy with rows sync hasn't got, on a branch of its own, is never switched away from.
    #[rstest]
    #[tokio::test]
    async fn a_copy_with_unsynced_rows_is_kept(dirs: Dirs) {
        let copy = tip(&["s1", "s2", "x1"], Some("x1"));
        let (source, session, machine) =
            setup(&dirs, HarnessKind::ClaudeCode, one_head(), Some(copy));
        let synced = resumer(&dirs, &machine).sync(&source, &session, None).await.unwrap();
        assert_eq!(synced.caught, Caught::Kept(Kept::Unsynced));
        assert!(!synced.holds(), "resumes, saying so");
        assert!(machine.appended.lock().is_empty());
    }

    /// Appending failing otherwise (the transcript changed meanwhile) keeps the copy, saying why.
    #[rstest]
    #[tokio::test]
    async fn a_failed_append_keeps_the_copy(dirs: Dirs) {
        let copy = tip(&["s1", "s2"], Some("s2"));
        let (source, session, mut machine) = setup(&dirs, HarnessKind::Pi, one_head(), Some(copy));
        machine.append_fails = Some(|| SyncError::Changed);
        let synced = resumer(&dirs, &machine).sync(&source, &session, None).await.unwrap();
        assert_eq!(
            synced.caught,
            Caught::Kept(Kept::Failed("the session's transcript changed since it was read".into()))
        );
        assert!(synced.holds());
    }
}
