//! Test fixtures: an in-memory [`SessionSource`] of realistic sessions, and a [`Resumer`] over a
//! fake filesystem.
//!
//! It covers all four harnesses, forks and subagents grouped under their roots, sessions from other
//! hosts, a deleted worktree (missing cwd), and live sessions (updated in the last two minutes).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use atuin_client::ai_session::{
    AiSessionDatabase, Analysis, AtuinSessionId, HarnessKind, HarnessSession, Message,
    NativeSessionId, SourceId,
};
use atuin_common::harnesstools::Harness as _;
use atuin_common::harnesstools::continuation::Flattened;
use atuin_common::harnesstools::rehydrate::{RehydrateMessage, RehydrateSession};
use atuin_common::harnesstools::resume::CwdRequirement;
use atuin_common::harnesstools::session::{Content, Role as Said, Usage};
use atuin_common::harnesstools::sync::{LocalTip, Stamp};
use atuin_domain::record::{HostId, RecordId};
use parking_lot::Mutex;
use time::{Duration, OffsetDateTime};

use super::ResumeContext;
use super::catchup::CatchUp;
use super::resumer::{
    Continued, ForkFrom, Forked, NotResumable, Restore, Resume, ResumeError, ResumePlan,
    ResumeTarget, Resumer, Switched,
};
use super::source::{Relation, SessionFilter, SessionPreview, SessionRow, SessionSource, Snippet};

pub const THIS_HOST_ID: &str = "01900000000070008000000000000001";
pub const REPO: &str = "/home/ellie/src/atuin";
/// A worktree that was deleted after the session ran.
pub const DELETED_WORKTREE: &str = "/home/ellie/src/atuin/.claude/worktrees/theme-preview";

/// The fixed "now" the fake sessions are relative to, so renders are deterministic.
pub fn now() -> OffsetDateTime {
    time::macros::datetime!(2026-09-27 15:00:00 UTC)
}

/// Where the fake picker runs: in the atuin repo on the `ai-resume` branch.
pub fn context() -> ResumeContext {
    ResumeContext {
        cwd: PathBuf::from(REPO),
        git_root: Some(PathBuf::from(REPO)),
        branch: Some("ai-resume".to_owned()),
        host_id: THIS_HOST_ID.to_owned(),
    }
}

/// A bare row with sensible defaults, for tests.
pub fn row(harness: HarnessKind, id: &str, title: &str) -> SessionRow {
    let started_at = now() - Duration::hours(1);
    SessionRow {
        atuin_id: atuin_id(harness, id, started_at),
        handle: handle(harness, id),
        parent: None,
        relation: Relation::Root,
        title: Snippet::plain(title),
        cwd: Some(PathBuf::from(REPO)),
        git_root: Some(PathBuf::from(REPO)),
        branch: Some("main".to_owned()),
        model: None,
        host_id: THIS_HOST_ID.to_owned(),
        started_at,
        updated_at: now() - Duration::minutes(30),
        active_at: now() - Duration::minutes(30),
        messages: 10,
        usage: Usage::default(),
        children: 0,
        matched: None,
    }
}

fn handle(harness: HarnessKind, id: &str) -> HarnessSession {
    HarnessSession {
        harness,
        session: NativeSessionId::from(id.to_owned()),
    }
}

/// A fake session's atuin id: a UUIDv7 at its start, like a minted one, but with its other bits
/// from its handle, so renders are deterministic.
fn atuin_id(harness: HarnessKind, id: &str, started_at: OffsetDateTime) -> AtuinSessionId {
    let millis = u64::try_from(started_at.unix_timestamp_nanos() / 1_000_000).unwrap_or(0);
    let hash = xxhash_rust::xxh3::xxh3_128(format!("{harness:?}:{id}").as_bytes()).to_le_bytes();
    let mut bits = [0; 10];
    bits.copy_from_slice(&hash[..10]);
    uuid::Builder::from_unix_timestamp_millis(millis, &bits).into_uuid().into()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Role {
    User,
    Assistant,
}

struct FakeSession {
    row: SessionRow,
    parent: Option<HarnessSession>,
    /// The top-most ancestor, as the sidecar groups sessions.
    root: Option<HarnessSession>,
    messages: Vec<(Role, &'static str)>,
}

pub struct FakeSource {
    sessions: Vec<FakeSession>,
    /// The rows sync holds of a session, for its branches ([`SessionSource::analyse`]).
    synced: HashMap<HarnessSession, Vec<Message>>,
}

/// The session [`synced_rows`] are of.
pub fn synced_handle() -> HarnessSession {
    HarnessSession {
        harness: HarnessKind::ClaudeCode,
        session: NativeSessionId::from("s".to_owned()),
    }
}

/// The rows sync holds of a Claude Code session: `a`-`b` on this host, then `c`-`d` on another;
/// `diverged`, this host went on from `b` with `x`-`y` too.
pub fn synced_rows(diverged: bool) -> Vec<Message> {
    synced_rows_of(&synced_handle(), diverged)
}

/// [`synced_rows`], of session `handle`.
pub fn synced_rows_of(handle: &HarnessSession, diverged: bool) -> Vec<Message> {
    let mut specs = vec![
        ("a", None, 1, 0, false),
        ("b", Some("a"), 1, 1, true),
        ("c", Some("b"), 2, 2, false),
        ("d", Some("c"), 2, 3, true),
    ];
    if diverged {
        specs.extend([("x", Some("b"), 1, 4, false), ("y", Some("x"), 1, 5, true)]);
    }
    specs.into_iter().map(|spec| synced_row(handle, spec)).collect()
}

/// A copy holding `known`, going on from `at`.
pub fn local_tip(known: &[&str], at: Option<&str>) -> LocalTip {
    LocalTip {
        native_path: PathBuf::from("/t/s.jsonl"),
        known_source_ids: known.iter().map(|s| (*s).to_owned()).collect::<HashSet<_>>(),
        merged: HashMap::new(),
        tip_source_id: at.map(str::to_owned),
        stamp: Stamp::of(b""),
        modified: None,
        cwd: None,
        unswitchable: None,
    }
}

/// The hosts' ids, but for their last digit.
const HOST_BASE: u128 = 0x0190_0000_0000_7000_8000_0000_0000_0000;

/// A row synced of session `handle`: `id`, hanging from `parent`, captured on host `host` (its
/// id's last digit: 1 is this host) `minutes` after the hour, a prompt or (`reply`) a reply.
pub fn synced_row(
    handle: &HarnessSession,
    (id, parent, host, minutes, reply): (&str, Option<&str>, u128, i64, bool),
) -> Message {
    let role = if reply {
        Said::Assistant
    } else {
        Said::User
    };
    let mut message = Message::builder()
        .id(RecordId(atuin_common::utils::uuid_v7()))
        .session(handle.clone())
        .source_id(SourceId::from(id.to_owned()))
        .parent_source_id(parent.map(|p| SourceId::from(p.to_owned())))
        .timestamp(now() - Duration::hours(1) + Duration::minutes(minutes))
        .role(role)
        .content(vec![Content::Text(format!("message {id}"))])
        .build();
    message.host = Some(HostId(uuid::Uuid::from_u128(HOST_BASE | host)));
    message
}

struct Spec {
    harness: HarnessKind,
    id: &'static str,
    title: &'static str,
    cwd: &'static str,
    branch: Option<&'static str>,
    model: &'static str,
    host: &'static str,
    age: Duration,
    duration: Duration,
    msgs: u64,
    messages: Vec<(Role, &'static str)>,
}

pub const OTHER_HOST_ID: &str = "01900000000070008000000000000002";
const OTHER_HOST: &str = OTHER_HOST_ID;
const LAPTOP: &str = "01900000000070008000000000000003";
const HERE: &str = THIS_HOST_ID;

fn build(spec: Spec, relation: Relation, parent: Option<&SessionRow>) -> FakeSession {
    let cwd = PathBuf::from(spec.cwd);
    let git_root = if spec.cwd.starts_with(REPO) {
        Some(PathBuf::from(REPO))
    } else if spec.cwd.starts_with("/home/ellie/src/") || spec.cwd.starts_with("/srv/ci/") {
        Some(cwd.clone())
    } else if spec.cwd.starts_with("/home/ellie/work/infra") {
        Some(PathBuf::from("/home/ellie/work/infra"))
    } else {
        None
    };
    let updated_at = now() - spec.age;
    // An untitled session is titled from its first prompt, as the sidecar source does.
    let title = if spec.title.is_empty() {
        let first = spec.messages.iter().find(|(role, _)| *role == Role::User);
        super::title::derive(first.map_or("", |(_, text)| text))
    } else {
        spec.title.to_owned()
    };
    FakeSession {
        row: SessionRow {
            atuin_id: atuin_id(spec.harness, spec.id, updated_at - spec.duration),
            handle: handle(spec.harness, spec.id),
            parent: parent.map(|p| p.handle.clone()),
            relation,
            title: Snippet::plain(title),
            cwd: Some(cwd),
            git_root,
            branch: spec.branch.map(str::to_owned),
            model: Some(spec.model.to_owned()),
            host_id: spec.host.to_owned(),
            started_at: updated_at - spec.duration,
            updated_at,
            active_at: updated_at,
            messages: spec.msgs,
            usage: Usage {
                input: Some(spec.msgs * 2_300),
                output: Some(spec.msgs * 410),
                cache_read: Some(spec.msgs * 19_000),
                cache_write: Some(spec.msgs * 1_200),
                reasoning: None,
            },
            children: 0,
            matched: None,
        },
        parent: parent.map(|p| p.handle.clone()),
        root: None,
        messages: spec.messages,
    }
}

impl Default for FakeSource {
    fn default() -> Self {
        Self::new()
    }
}

impl FakeSource {
    #[allow(clippy::too_many_lines)]
    pub fn new() -> Self {
        use HarnessKind::{ClaudeCode, Codex, Opencode, Pi};
        use Role::{Assistant as A, User as U};

        let mut sessions = Vec::new();
        let m = Duration::minutes;
        let h = Duration::hours;
        let d = Duration::days;

        let resume = build(
            Spec {
                harness: ClaudeCode,
                id: "7f3c9a12-5be0-4d7e-9c41-0a8e2b6f4d10",
                title: "Add an interactive resume picker to atuin ai",
                cwd: REPO,
                branch: Some("ai-resume"),
                model: "claude-opus-4-5",
                host: HERE,
                age: Duration::seconds(30),
                duration: h(3),
                msgs: 142,
                messages: vec![
                    (
                        U,
                        "Build `atuin ai resume`: a picker over captured agent sessions that \
                         looks like **history search**.\n\n\
                         - a preview pane with the first prompt and the last reply\n\
                         - resume on `enter`, edit on `tab`\n\
                         - keep the vim and emacs keymaps",
                    ),
                    (
                        A,
                        "I'll start by extracting the keymap and cursor plumbing from the history \
                         search so both TUIs can share it.",
                    ),
                    (
                        U,
                        "Group forks and subagents under their root session, the list floods \
                         otherwise.",
                    ),
                    (
                        A,
                        "## Grouping done\n\n\
                         Rows now fold forks and subagents into their root and show `+N`:\n\n\
                         | Harness | Forks | Subagents |\n\
                         |:--|--:|--:|\n\
                         | Claude Code | 1 | 3 |\n\
                         | Codex | 0 | 0 |\n\n\
                         ```rust\n\
                         row.children = u32::try_from(group.len() - 1).unwrap_or(u32::MAX);\n\
                         ```\n\n\
                         Next up (see [the design notes](https://docs.atuin.sh/ai/resume)):\n\n\
                         1. the **Inspect** tab with the resume command\n\
                         2. the children list, as a tree\n\n\
                         > Tool calls and reasoning stay out of the preview.",
                    ),
                ],
            },
            Relation::Root,
            None,
        );
        let explore = build(
            Spec {
                harness: ClaudeCode,
                id: "agent-a1b2c3d4",
                title: "Explore: find the history search keybinding code",
                cwd: REPO,
                branch: Some("ai-resume"),
                model: "claude-haiku-4-5",
                host: HERE,
                age: m(50),
                duration: m(4),
                msgs: 18,
                messages: vec![
                    (U, "Find where the history search resolves keys to actions."),
                    (
                        A,
                        "Keys resolve in keybindings/keymap.rs; conditions like cursor-at-start \
                         are evaluated in conditions.rs.",
                    ),
                ],
            },
            Relation::Subagent,
            Some(&resume.row),
        );
        let review = build(
            Spec {
                harness: ClaudeCode,
                id: "agent-e5f6a7b8",
                // Untitled, as subagents are: the title comes from the brief.
                title: "",
                cwd: REPO,
                branch: Some("ai-resume"),
                model: "claude-sonnet-4-5",
                host: HERE,
                age: m(12),
                duration: m(6),
                msgs: 9,
                messages: vec![
                    (
                        U,
                        "You're working on `atuin ai resume` in a git worktree of \
                         /home/ellie/src/atuin. Repo: /home/ellie/src/atuin (Rust). Review the \
                         resume picker diff for **history search** regressions. Report back \
                         briefly.",
                    ),
                    (A, "No behaviour changes in history search; all 297 tests still pass."),
                ],
            },
            Relation::Subagent,
            Some(&resume.row),
        );
        let fork = build(
            Spec {
                harness: ClaudeCode,
                id: "0c1d2e3f-8a9b-4c5d-9e0f-112233445566",
                title: "Add an interactive resume picker to atuin ai (fork)",
                cwd: REPO,
                branch: Some("ai-resume"),
                model: "claude-opus-4-5",
                host: HERE,
                age: h(2),
                duration: m(40),
                msgs: 61,
                messages: vec![
                    (U, "Try rendering the rows with ratatui's Table instead."),
                    (
                        A,
                        "Table can't highlight spans inside a cell cleanly; reverting to the \
                         custom list.",
                    ),
                ],
            },
            Relation::Fork,
            Some(&resume.row),
        );
        let fork_explore = build(
            Spec {
                harness: ClaudeCode,
                id: "agent-c9d0e1f2",
                title: "Explore: how ratatui's Table highlights cells",
                cwd: REPO,
                branch: Some("ai-resume"),
                model: "claude-haiku-4-5",
                host: HERE,
                age: h(2) + m(20),
                duration: m(3),
                msgs: 11,
                messages: vec![
                    (U, "Can a Table cell carry several styled spans?"),
                    (A, "Yes, a Cell takes a Line, but highlight_style restyles the whole row."),
                ],
            },
            Relation::Subagent,
            Some(&fork.row),
        );
        sessions.extend([resume, explore, review, fork, fork_explore]);

        sessions.push(build(
            Spec {
                harness: Codex,
                id: "019a6b2e-77c1-7d20-b4f3-5e8d9c0a1b2c",
                title: "Explain the daemon gRPC reconnect logic",
                cwd: "/home/ellie/src/atuin/crates/atuin-daemon",
                branch: Some("main"),
                model: "gpt-5-codex",
                host: HERE,
                age: Duration::seconds(95),
                duration: m(20),
                msgs: 27,
                messages: vec![
                    (U, "Why does the daemon client reconnect twice after a restart?"),
                    (
                        A,
                        "The first retry races the socket file being recreated; the backoff only \
                         starts after the second failure.",
                    ),
                ],
            },
            Relation::Root,
            None,
        ));
        sessions.push(build(
            Spec {
                harness: Codex,
                id: "019a5f10-1c2d-7e3f-8a4b-c5d6e7f8a9b0",
                title: "Fix the flaky sync test in the record store",
                cwd: REPO,
                branch: Some("main"),
                model: "gpt-5-codex",
                host: HERE,
                age: h(1),
                duration: m(45),
                msgs: 58,
                messages: vec![
                    (U, "`record::sync::tests::sync_down` is flaky on CI, can you find out why?"),
                    (
                        A,
                        "It depends on **wall-clock ordering**: two records written in the same \
                         millisecond sort by id, not by time.\n\nI switched the test to a fixed \
                         clock:\n\n```rust\nlet clock = FixedClock::at(datetime!(2026-01-01 0:00 \
                         UTC));\nstore.push_at(&record, clock.tick()).await?;\n```\n\nIt passed \
                         *500 runs* in a row.",
                    ),
                ],
            },
            Relation::Root,
            None,
        ));
        let wal = build(
            Spec {
                harness: Opencode,
                id: "ses_4b8e2f1a9c3d7e6f",
                title: "Move the sidecar database to WAL and open it read-only",
                cwd: REPO,
                branch: Some("ai-sidecar"),
                model: "anthropic/claude-sonnet-4-5",
                host: HERE,
                age: h(3),
                duration: h(1),
                msgs: 33,
                messages: vec![
                    (
                        U,
                        "The picker must paint in under 50ms. Open the sidecar read-only with WAL.",
                    ),
                    (
                        A,
                        "Opened with mode=ro and journal_mode=WAL; first query now takes 6ms on a \
                         warm cache.",
                    ),
                ],
            },
            Relation::Root,
            None,
        );
        let wal_fork = build(
            Spec {
                harness: Opencode,
                id: "ses_4b8e2f1a9c3d7e70",
                title: "Move the sidecar database to WAL (try mmap)",
                cwd: REPO,
                branch: Some("ai-sidecar"),
                model: "anthropic/claude-sonnet-4-5",
                host: HERE,
                age: h(4),
                duration: m(15),
                msgs: 12,
                messages: vec![
                    (U, "Does mmap_size help the cold start?"),
                    (A, "Only by about 1ms. Not worth the extra pragma."),
                ],
            },
            Relation::Fork,
            Some(&wal.row),
        );
        sessions.extend([wal, wal_fork]);

        sessions.push(build(
            Spec {
                harness: ClaudeCode,
                id: "b2c4d6e8-0a1b-4c3d-8e5f-6a7b8c9d0e1f",
                title: "Plan the dotfiles sync redesign",
                cwd: "/home/ellie/src/dotfiles",
                branch: Some("main"),
                model: "claude-opus-4-5",
                host: HERE,
                age: h(5),
                duration: h(2),
                msgs: 70,
                messages: vec![
                    (U, "Sketch how dotfiles sync should handle conflicting aliases."),
                    (A, "Last writer wins per alias, with a conflict log you can review."),
                ],
            },
            Relation::Root,
            None,
        ));
        sessions.push(build(
            Spec {
                harness: ClaudeCode,
                id: "c3d5e7f9-1b2c-4d3e-9f6a-7b8c9d0e1f2a",
                title: "Review PR #4277: remove the replaced GitHub workflows",
                cwd: REPO,
                branch: Some("main"),
                model: "claude-sonnet-4-5",
                host: HERE,
                age: h(8),
                duration: m(25),
                msgs: 40,
                messages: vec![
                    (U, "Review #4277 and check nothing still references the deleted workflows."),
                    (
                        A,
                        "Two badges in README.md still point at the old workflows; everything \
                         else is clean.",
                    ),
                ],
            },
            Relation::Root,
            None,
        ));
        sessions.push(build(
            Spec {
                harness: Pi,
                id: "5e6f7a8b-9c0d-4e1f-a2b3-c4d5e6f7a8b9",
                title: "Benchmark the FTS prefix index size",
                cwd: REPO,
                branch: Some("main"),
                model: "gpt-5",
                host: HERE,
                age: d(1),
                duration: h(1),
                msgs: 21,
                messages: vec![
                    (U, "How much bigger does the FTS index get with prefix='2 3'?"),
                    (
                        A,
                        "On 180k messages: 41MB to 58MB. prefix='3' alone costs 9MB and covers \
                         most typing.",
                    ),
                ],
            },
            Relation::Root,
            None,
        ));
        sessions.push(build(
            Spec {
                harness: ClaudeCode,
                id: "d4e6f8a0-2c3d-4e4f-8a7b-8c9d0e1f2a3b",
                title: "Bisect the aarch64 release build failure",
                cwd: "/srv/ci/atuin",
                branch: Some("main"),
                model: "claude-opus-4-5",
                host: OTHER_HOST,
                age: d(2),
                duration: h(4),
                msgs: 77,
                messages: vec![
                    (U, "The aarch64 musl release build fails at link time. Bisect it."),
                    (A, "First bad commit bumps ring; pinning cc to 1.2.30 fixes the link error."),
                ],
            },
            Relation::Root,
            None,
        ));
        sessions.push(build(
            Spec {
                harness: Codex,
                id: "019a3c4d-5e6f-7a8b-9c0d-1e2f3a4b5c6d",
                title: "Speed up the history search first paint",
                cwd: REPO,
                branch: Some("perf-first-paint"),
                model: "gpt-5-codex",
                host: HERE,
                age: d(2) + h(3),
                duration: h(2),
                msgs: 49,
                messages: vec![
                    (U, "The first frame of atuin search takes 300ms on a cold cache. Why?"),
                    (
                        A,
                        "The history count was a full table scan on the UI path; it now runs in \
                         the background.",
                    ),
                ],
            },
            Relation::Root,
            None,
        ));
        sessions.push(build(
            Spec {
                harness: Pi,
                id: "e5f7a9b1-3d4e-4f5a-9b8c-9d0e1f2a3b4c",
                title: "Prototype a live theme preview",
                cwd: DELETED_WORKTREE,
                branch: Some("theme-preview"),
                model: "gpt-5",
                host: HERE,
                age: d(4),
                duration: h(1),
                msgs: 36,
                messages: vec![
                    (U, "Can atuin theme preview render the search UI with a candidate theme?"),
                    (
                        A,
                        "It can, with a fake history list; the prototype is on the theme-preview \
                         branch.",
                    ),
                ],
            },
            Relation::Root,
            None,
        ));
        sessions.push(build(
            Spec {
                harness: Opencode,
                id: "ses_9a8b7c6d5e4f3a2b",
                title: "Debug the ingress 502s after the node pool upgrade",
                cwd: "/home/ellie/work/infra/k8s",
                branch: Some("main"),
                model: "openai/gpt-5",
                host: LAPTOP,
                age: d(6),
                duration: h(2),
                msgs: 55,
                messages: vec![
                    (U, "Ingress returns 502 for about a minute after every node pool upgrade."),
                    (
                        A,
                        "The pods have no preStop hook, so they drop connections before \
                         deregistering.",
                    ),
                ],
            },
            Relation::Root,
            None,
        ));
        sessions.push(build(
            Spec {
                harness: ClaudeCode,
                id: "f6a8b0c2-4e5f-4a6b-8c9d-0e1f2a3b4c5d",
                title: "Draft the v18.13 release notes",
                cwd: REPO,
                branch: Some("main"),
                model: "claude-opus-4-5",
                host: HERE,
                age: d(9),
                duration: m(50),
                msgs: 31,
                messages: vec![
                    (U, "Draft release notes from the merged PRs since v18.12."),
                    (A, "Drafted, grouped into features, fixes and internal changes."),
                ],
            },
            Relation::Root,
            None,
        ));
        sessions.push(build(
            Spec {
                harness: Opencode,
                id: "ses_1f2e3d4c5b6a7980",
                title: "Tidy the clippy lints in atuin-client",
                cwd: "/home/ellie/src/atuin/crates/atuin-client",
                branch: Some("main"),
                model: "anthropic/claude-haiku-4-5",
                host: HERE,
                age: d(12),
                duration: m(35),
                msgs: 24,
                messages: vec![
                    (U, "Fix every clippy warning in atuin-client without allow attributes."),
                    (A, "Fixed 41 warnings; two needed an allow for should_implement_trait."),
                ],
            },
            Relation::Root,
            None,
        ));
        sessions.push(build(
            Spec {
                harness: Pi,
                id: "6f7a8b9c-0d1e-4f2a-b3c4-d5e6f7a8b9c0",
                title: "Why does this borrow outlive the loop?",
                cwd: "/home/ellie",
                branch: None,
                model: "claude-sonnet-4-5",
                host: HERE,
                age: d(15),
                duration: m(10),
                msgs: 8,
                messages: vec![
                    (U, "Why does this borrow outlive the loop body?"),
                    (
                        A,
                        "The iterator holds the borrow until it's dropped; collect first, then \
                         mutate.",
                    ),
                ],
            },
            Relation::Root,
            None,
        ));

        // Group every session under its top-most ancestor, as the sidecar does.
        let parents: Vec<(HarnessSession, Option<HarnessSession>)> =
            sessions.iter().map(|s| (s.row.handle.clone(), s.parent.clone())).collect();
        for s in &mut sessions {
            let mut root = s.parent.clone();
            while let Some(up) = root
                .as_ref()
                .and_then(|r| parents.iter().find(|(h, _)| h == r))
                .and_then(|(_, p)| p.clone())
            {
                root = Some(up);
            }
            s.root = root;
        }

        Self {
            sessions,
            synced: HashMap::new(),
        }
    }

    /// This source, with `rows` synced of session `handle`.
    pub fn with_synced(mut self, handle: &HarnessSession, rows: Vec<Message>) -> Self {
        self.synced.insert(handle.clone(), rows);
        self
    }

    /// A source holding only `rows`.
    #[cfg(test)]
    pub fn from_rows(rows: Vec<SessionRow>) -> Self {
        Self {
            sessions: rows
                .into_iter()
                .map(|row| FakeSession {
                    row,
                    parent: None,
                    root: None,
                    messages: Vec::new(),
                })
                .collect(),
            synced: HashMap::new(),
        }
    }

    /// Every row synced of `session`, in time order, as the sidecar restores them.
    fn synced_messages(&self, session: &HarnessSession) -> Vec<RehydrateMessage> {
        let mut rows = self.synced.get(session).cloned().unwrap_or_default();
        rows.sort_by_key(|m| m.timestamp);
        rows.into_iter().map(RehydrateMessage::from).collect()
    }

    fn children_of<'a>(
        &'a self,
        root: &'a HarnessSession,
    ) -> impl Iterator<Item = &'a FakeSession> {
        self.sessions.iter().filter(move |s| s.root.as_ref() == Some(root))
    }

    fn is_row(s: &FakeSession, filter: &SessionFilter) -> bool {
        if filter.db.roots_only {
            s.row.relation == Relation::Root
        } else {
            s.row.relation.is_listed()
        }
    }

    fn matches_scope(row: &SessionRow, filter: &SessionFilter) -> bool {
        let filter = &filter.db;
        let cwd = row.cwd.as_deref();
        filter.harness.is_none_or(|h| h == row.handle.harness)
            && filter.model.as_ref().is_none_or(|m| {
                row.model.as_ref().is_some_and(|rm| rm.to_lowercase().contains(&m.to_lowercase()))
            })
            && filter.branch.as_ref().is_none_or(|b| row.branch.as_ref() == Some(b))
            && filter.host.is_none_or(|h| row.host_id == h.0.as_simple().to_string())
            && filter.workspace.as_deref().is_none_or(|p| cwd.is_some_and(|c| c.starts_with(p)))
            && filter.directory.as_deref().is_none_or(|p| cwd == Some(p))
    }
}

/// Byte ranges of every case-insensitive occurrence of `terms` in `text` (ASCII case folding, so
/// byte offsets stay valid).
fn find_all(text: &str, terms: &[String]) -> Vec<std::ops::Range<usize>> {
    let lower = text.to_ascii_lowercase();
    let ranges = terms
        .iter()
        .flat_map(|t| lower.match_indices(t.as_str()).map(|(i, m)| i..i + m.len()))
        .collect();
    super::title::merge(ranges)
}

/// A window of `text` around its first match, with highlights relative to the window.
fn snippet(text: &str, terms: &[String]) -> Option<Snippet> {
    let ranges = find_all(text, terms);
    let first = ranges.first()?.start;
    // Start at a word boundary a little before the match.
    let mut start = first.saturating_sub(30);
    while !text.is_char_boundary(start) {
        start -= 1;
    }
    if start > 0 {
        start = text[start..first].find(' ').map_or(first, |i| start + i + 1);
    }
    let mut end = (first + 90).min(text.len());
    while !text.is_char_boundary(end) {
        end += 1;
    }
    let highlights = ranges
        .into_iter()
        .filter(|r| r.start >= start && r.end <= end)
        .map(|r| r.start - start..r.end - start)
        .collect();
    Some(Snippet {
        text: text[start..end].to_owned(),
        highlights,
    })
}

#[async_trait]
impl SessionSource for FakeSource {
    async fn search(&self, filter: &SessionFilter) -> eyre::Result<Vec<SessionRow>> {
        let terms: Vec<String> =
            filter.text.split_whitespace().map(str::to_ascii_lowercase).collect();

        let mut rows: Vec<(usize, SessionRow)> = Vec::new();
        for s in self.sessions.iter().filter(|s| Self::is_row(s, filter)) {
            if !Self::matches_scope(&s.row, filter) {
                continue;
            }
            let group: Vec<&FakeSession> = if s.row.relation == Relation::Root {
                std::iter::once(s).chain(self.children_of(&s.row.handle)).collect()
            } else {
                vec![s]
            };
            let mut row = s.row.clone();
            row.children = u32::try_from(group.len() - 1).unwrap_or(u32::MAX);
            row.updated_at = group.iter().map(|g| g.row.updated_at).max().unwrap_or(row.updated_at);

            let mut score = 0;
            if !terms.is_empty() {
                let texts = group
                    .iter()
                    .flat_map(|g| {
                        std::iter::once(g.row.title.text.as_str())
                            .chain(g.messages.iter().map(|(_, t)| *t))
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
                    .to_ascii_lowercase();
                if !terms.iter().all(|t| texts.contains(t.as_str())) {
                    continue;
                }
                score = find_all(&texts, &terms).len();
                row.title.highlights = find_all(&row.title.text, &terms);
                row.matched = group
                    .iter()
                    .flat_map(|g| g.messages.iter().map(|(_, t)| *t))
                    .find_map(|t| snippet(t, &terms));
            }
            rows.push((score, row));
        }

        rows.sort_by(|(sa, a), (sb, b)| sb.cmp(sa).then(b.updated_at.cmp(&a.updated_at)));
        let mut rows: Vec<SessionRow> = rows.into_iter().map(|(_, r)| r).collect();
        if filter.limit != 0 {
            rows.truncate(filter.limit);
        }
        Ok(rows)
    }

    async fn find_by_id(&self, id: &str) -> eyre::Result<Vec<SessionRow>> {
        Ok(self
            .sessions
            .iter()
            .filter(|s| {
                s.row.handle.session.as_ref().starts_with(id) || s.row.atuin_id.has_prefix(id)
            })
            .map(|s| s.row.clone())
            .collect())
    }

    async fn preview(&self, session: &HarnessSession) -> eyre::Result<SessionPreview> {
        let Some(s) = self.sessions.iter().find(|s| &s.row.handle == session) else {
            return Ok(SessionPreview::default());
        };
        Ok(SessionPreview {
            first_prompt: s
                .messages
                .iter()
                .find(|(r, _)| *r == Role::User)
                .map(|(_, t)| (*t).to_owned()),
            last_assistant: s
                .messages
                .iter()
                .rev()
                .find(|(r, _)| *r == Role::Assistant)
                .map(|(_, t)| (*t).to_owned()),
        })
    }

    async fn children(&self, session: &HarnessSession) -> eyre::Result<Vec<SessionRow>> {
        let mut rows: Vec<SessionRow> = self
            .children_of(session)
            .filter(|s| s.row.relation.is_listed())
            .map(|s| s.row.clone())
            .collect();
        rows.sort_by_key(|r| std::cmp::Reverse(r.updated_at));
        Ok(rows)
    }

    /// The session, with every row synced of it as its messages (the fake sessions' own messages
    /// are preview text only).
    async fn rehydrate(
        &self,
        session: &HarnessSession,
        cwd: &Path,
    ) -> eyre::Result<RehydrateSession> {
        let row = &self
            .sessions
            .iter()
            .find(|s| &s.row.handle == session)
            .ok_or_else(|| eyre::eyre!("no such session"))?
            .row;
        Ok(RehydrateSession {
            id: row.handle.session.to_string(),
            title: Some(row.title.text.clone()),
            cwd: cwd.to_owned(),
            original_cwd: row.cwd.clone(),
            git_branch: row.branch.clone(),
            model: row.model.clone(),
            started_at: row.started_at,
            messages: self.synced_messages(session),
            fork_of: None,
        })
    }

    async fn analyse(&self, session: &HarnessSession) -> eyre::Result<Option<Analysis>> {
        let Some(rows) = self.synced.get(session) else {
            return Ok(None);
        };
        let db = AiSessionDatabase::in_memory().await?;
        for row in rows {
            db.append(row).await?;
        }
        Ok(Some(db.analyse(session).await?))
    }
}

/// Where [`FakeResumer::switch`] says the copy switched is kept.
pub const SWITCHED_BACKUP: &str = "/data/ai/switched/claude-code/s1-20260918T100000.000Z.jsonl";

/// A [`Resumer`] for the fake sessions: the harnesses' real plans, with the deleted worktree as the
/// only missing directory (none of the fake paths exist on this machine, so it can't check).
/// Other hosts' sessions are restored from sync, into the directory they ran in; restoring
/// writes nothing.
pub struct FakeResumer {
    pub missing: HashSet<PathBuf>,
    /// What catching this host's sessions up does, when they are caught up at all.
    pub catch_up: Option<CatchUp>,
    /// What each fork started from.
    pub forks: Arc<Mutex<Vec<ForkFrom>>>,
    /// The head each switch went to.
    pub switches: Arc<Mutex<Vec<SourceId>>>,
}

impl Default for FakeResumer {
    fn default() -> Self {
        Self {
            missing: HashSet::from([PathBuf::from(DELETED_WORKTREE)]),
            catch_up: None,
            forks: Arc::default(),
            switches: Arc::default(),
        }
    }
}

impl FakeResumer {
    fn plan_for(
        &self,
        session: &SessionRow,
        native: Option<PathBuf>,
    ) -> Result<ResumePlan, NotResumable> {
        let kind = session.handle.harness;
        let harness = kind.harness().ok_or(NotResumable::Unsupported("this"))?;
        let mut target = ResumeTarget::new(session.handle.session.as_ref());
        if let Some(cwd) = &session.cwd {
            target = target.with_cwd(cwd);
        }
        if let Some(native) = native {
            target = target.with_native_path(native);
        }
        let mut plan = harness.resume(&target, None)?;
        // `ResumePlan::prepare`, against the fake filesystem.
        if plan.cwd.as_ref().is_some_and(|c| self.missing.contains(c)) {
            if plan.cwd_requirement == CwdRequirement::Required {
                return Err(ResumeError::CwdMissing(plan.cwd).into());
            }
            plan.cwd = None;
        }
        Ok(plan)
    }
}

#[async_trait]
impl Resumer for FakeResumer {
    /// This host's sessions are here, live while they were written to in the last two minutes;
    /// other hosts' are restored.
    async fn plan(&self, session: &SessionRow) -> Result<Resume, NotResumable> {
        let plan = self.plan_for(session, None)?;
        if session.host_id == THIS_HOST_ID {
            let ago = now() - session.active_at;
            return Ok(Resume {
                catch_up: self.catch_up.is_some(),
                live: ago < Duration::seconds(i64::try_from(super::state::LIVE_SECS).unwrap_or(0)),
                ..Resume::ready(plan)
            });
        }
        let cwd = session.cwd.clone().unwrap_or_else(|| PathBuf::from(REPO));
        Ok(Resume {
            restore: Some(Restore { cwd, note: None }),
            ..Resume::ready(plan)
        })
    }

    async fn catch_up(
        &self,
        _source: &dyn SessionSource,
        session: &SessionRow,
        _head: Option<&SourceId>,
    ) -> Result<CatchUp, NotResumable> {
        match &self.catch_up {
            Some(caught) if session.host_id == THIS_HOST_ID => Ok(caught.clone()),
            _ => Ok(CatchUp::Ready {
                plan: self.plan(session).await?.plan,
                status: None,
            }),
        }
    }

    async fn restore(
        &self,
        _source: &dyn SessionSource,
        session: &SessionRow,
        _restore: &Restore,
    ) -> Result<ResumePlan, NotResumable> {
        let native = PathBuf::from(format!("/restored/{}.jsonl", session.handle.session));
        self.plan_for(session, Some(native))
    }

    /// Every other harness is installed. A session of a harness atuin can't read back (Copilot)
    /// can't be continued anywhere.
    fn continue_targets(&self, session: &SessionRow) -> Vec<HarnessKind> {
        if session.handle.harness.harness().is_none() {
            return Vec::new();
        }
        [HarnessKind::ClaudeCode, HarnessKind::Codex, HarnessKind::Opencode, HarnessKind::Pi]
            .into_iter()
            .filter(|kind| *kind != session.handle.harness)
            .collect()
    }

    /// Plans resuming a made-up new session, written nowhere, with a round number of calls
    /// flattened.
    async fn continue_in(
        &self,
        _source: &dyn SessionSource,
        session: &SessionRow,
        target: HarnessKind,
    ) -> Result<Continued, NotResumable> {
        let mut row = session.clone();
        row.handle = HarnessSession {
            harness: target,
            session: NativeSessionId::from(format!("continued-{}", session.handle.session)),
        };
        let native = PathBuf::from(format!("/continued/{}", row.handle.session));
        Ok(Continued {
            target,
            plan: self.plan_for(&row, Some(native))?,
            flattened: Flattened {
                tool_calls: 42,
                tool_results: 42,
                reasoning: 7,
            },
            note: None,
        })
    }

    /// Every harness atuin writes is installed.
    fn can_fork(&self, session: &SessionRow) -> bool {
        session.handle.harness.harness().is_some()
    }

    /// Plans resuming a made-up fork, written nowhere.
    async fn fork(
        &self,
        _source: &dyn SessionSource,
        session: &SessionRow,
        from: ForkFrom,
    ) -> Result<Forked, NotResumable> {
        self.forks.lock().push(from);
        let mut row = session.clone();
        row.handle.session = NativeSessionId::from(format!("forked-{}", session.handle.session));
        let native = PathBuf::from(format!("/forked/{}", row.handle.session));
        Ok(Forked {
            harness: row.handle.harness,
            id: row.handle.session.to_string(),
            plan: self.plan_for(&row, Some(native))?,
            note: None,
        })
    }

    /// Plans resuming the copy as switched, writing nothing.
    async fn switch(
        &self,
        _source: &dyn SessionSource,
        session: &SessionRow,
        head: Option<&SourceId>,
    ) -> Result<Switched, NotResumable> {
        let head = head.cloned().ok_or(NotResumable::Switch("no head named".to_owned()))?;
        self.switches.lock().push(head);
        Ok(Switched {
            host: "@00000002".to_owned(),
            messages: 4,
            backup: PathBuf::from(SWITCHED_BACKUP),
            warning: None,
            plan: self.plan_for(session, None)?,
        })
    }
}
