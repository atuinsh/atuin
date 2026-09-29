//! Catching this machine's copy of a session up with sync before resuming it, the git way.
//!
//! A session keeps one id on every machine. Each machine's native transcript is a branch of it
//! and the synced rows are the remote ([`atuin_common::harnesstools::sync`]). Resuming a
//! session here brings the transcript to the head the user resumes (the newest, unless they
//! picked another branch of a [diverged](SessionRow::diverged) session):
//!
//! - no transcript here: it is written out from sync, along that head's branch;
//! - the transcript is at the head, or past it: resumed as it is;
//! - the transcript is behind, on the head's branch: the rows it lacks are appended (a
//!   fast-forward), and it resumes in place;
//! - the transcript is on another branch: the head's branch is appended beside it and made the
//!   one the harness continues from, in the same session. Never merged, and nothing is written
//!   again under a new id, unless the harness can't take the branch in place (opencode keeps a
//!   session as one line): then it is continued as a new session linked to this one.
//!
//! Nothing is written under a harness that has the session open on this machine, and a copy
//! holding messages sync hasn't got is never switched away from.
//!
//! This module holds the decisions and what they are called; [`super::resumer`] carries them
//! out against the harness.

use std::collections::HashSet;

use atuin_client::ai_session::{HarnessKind, Head};
use atuin_common::harnesstools::continuation::Flattened;
use atuin_common::harnesstools::rehydrate::{Flatten, RehydrateMessage, flatten_uncaptured_calls};
use atuin_common::harnesstools::session::{Content, Role};
use atuin_common::harnesstools::sync::LocalTip;
use time::{Duration, OffsetDateTime};

use super::resumer::ResumePlan;
use super::source::{SessionRow, harness_label};

/// A branch whose newest row another host wrote this recently may still be going on there: resuming
/// it here warns that it will branch the session. Bounded below by sync latency: a host still
/// working reads as idle until its rows arrive, which takes a sync interval or two.
pub const LIVE_ELSEWHERE: Duration = Duration::minutes(5);

/// What bringing this machine's copy of a session to a head does, as [`classify`] decides it.
#[derive(Debug, Clone)]
pub enum Step {
    /// The copy is at the head: resume it as it is.
    AsIs,
    /// The copy holds the head and went on past it (with rows sync may not have yet): resume it
    /// as it is.
    Ahead,
    /// The copy is on a branch sync doesn't know (rows it hasn't got): resume it as it is,
    /// never switching away from them.
    Unsynced,
    /// The copy is behind on the head's branch: append these rows.
    FastForward(Vec<RehydrateMessage>),
    /// The copy is on another branch: append the head's branch past where they part (these
    /// rows), beside it, and make it the tip.
    Branch(Vec<RehydrateMessage>),
    /// The copy already holds the head's branch, but continues from another: make the head the
    /// tip.
    Switch,
}

/// Whether a row makes a branch worth telling apart: a prompt, or assistant text (as the sidecar
/// decides which forks are branches; tool calls, attachments and compactions aren't).
fn substantive(row: &RehydrateMessage) -> bool {
    match row.role {
        Role::User => true,
        Role::Assistant => {
            row.content.iter().any(|c| matches!(c, Content::Text(t) if !t.trim().is_empty()))
        }
        _ => false,
    }
}

/// The rows of `path` after the last one `tip` holds: all of them when it holds none.
///
/// A transcript written from sync holds no line for a row its writer merged into the line before
/// ([`flatten_uncaptured_calls`]: calls captured without their input become notes in their
/// turn's text), so with `merged` (the session's harness, whose writer merged along `path`:
/// the copy is on it), such rows right after the last line it holds are in it already, and are
/// left out.
fn missing(
    path: &[RehydrateMessage],
    tip: &LocalTip,
    merged: Option<HarnessKind>,
) -> Vec<RehydrateMessage> {
    let Some(last) = path.iter().rposition(|m| tip.known_source_ids.contains(&m.source_id)) else {
        return path.to_vec();
    };
    let Some(harness) = merged else {
        return path[last + 1..].to_vec();
    };
    let flattened = match harness {
        HarnessKind::ClaudeCode | HarnessKind::Pi => flatten_uncaptured_calls(path, &Flatten::Runs),
        _ => flatten_uncaptured_calls(path, &Flatten::Notes {
            host: &|_, _| true,
            own: &|_| true,
        }),
    };
    let from =
        (last + 1..path.len()).find(|&n| !flattened[n].content.is_empty()).unwrap_or(path.len());
    // What hung from a merged row hangs from the line it went into, as the writer wrote it.
    let merged: HashSet<&str> = path[last + 1..from].iter().map(|m| m.source_id.as_str()).collect();
    let into = &path[last].source_id;
    let mut rows = path[from..].to_vec();
    for row in &mut rows {
        if row.parent_source_id.as_deref().is_some_and(|p| merged.contains(p)) {
            row.parent_source_id = Some(into.clone());
        }
    }
    rows
}

/// How to bring the copy read as `tip` to `head`, whose branch (root to head) is `path`.
///
/// `tip_path` is the branch the copy's own tip is on, as sync has it (empty when sync doesn't
/// hold that row); only needed when the tip isn't on `path`. `picked`: the user picked this head
/// among several, so a copy holding it is switched to it even when it continues from an earlier
/// row of it. `harness` is the session's, for the rows its writer merges (see [`missing`]).
pub fn classify(
    tip: &LocalTip,
    head: &str,
    path: &[RehydrateMessage],
    tip_path: Option<&[RehydrateMessage]>,
    picked: bool,
    harness: HarnessKind,
) -> Step {
    let on_path =
        tip.tip_source_id.as_deref().is_none_or(|at| path.iter().any(|m| m.source_id == at));
    let missing = missing(path, tip, on_path.then_some(harness));
    let Some(at) = tip.tip_source_id.as_deref() else {
        // A transcript the harness would start empty.
        return if missing.is_empty() {
            Step::AsIs
        } else {
            Step::FastForward(missing)
        };
    };
    if at == head {
        return Step::AsIs;
    }
    if path.iter().any(|m| m.source_id == at) {
        return match missing.is_empty() {
            false => Step::FastForward(missing),
            // It holds the head, but continues from before it: a rewind here.
            true if picked => Step::Switch,
            true => Step::AsIs,
        };
    }
    let holds_head = tip.known_source_ids.contains(head);
    let tip_path = tip_path.unwrap_or_default();
    if tip_path.is_empty() {
        return if holds_head {
            Step::Ahead
        } else {
            Step::Unsynced
        };
    }
    if tip_path.iter().any(|m| m.source_id == head) {
        return Step::Ahead;
    }
    let shared = path.iter().zip(tip_path).take_while(|(a, b)| a.source_id == b.source_id).count();
    if !tip_path[shared..].iter().any(substantive) {
        // A twig off the head's branch (a tool call, an attachment): not a branch of its own.
        return if missing.is_empty() {
            Step::AsIs
        } else {
            Step::FastForward(missing)
        };
    }
    if missing.is_empty() {
        Step::Switch
    } else {
        Step::Branch(missing)
    }
}

/// Why nothing was written, and this machine's copy resumes as it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kept {
    /// A harness on this machine has the session open, or may have.
    Live {
        pid: Option<u32>,
    },
    /// The copy holds messages sync hasn't got, on a branch of its own.
    Unsynced,
    /// Catching up failed.
    Failed(String),
}

/// What catching up did (see the module docs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Caught {
    /// Nothing to do: the copy here is at the head, or past it.
    UpToDate,
    /// There was no copy here: `rows` messages were written out from sync. `note` says where it
    /// resumes when that isn't where it ran.
    Restored {
        rows: usize,
        note: Option<String>,
    },
    /// `rows` messages appended to a copy that was behind.
    FastForwarded {
        rows: usize,
    },
    /// The head's branch made the one the session continues from, in the same session: `rows`
    /// messages appended beside this machine's (none when it already held them).
    Switched {
        rows: usize,
    },
    /// Nothing written: the copy here resumes as it is.
    Kept(Kept),
    /// The harness can't take the branch in place (`why`): it was continued as a new session
    /// linked to this one, which is what resumes.
    Forked {
        why: String,
        flattened: Flattened,
    },
}

/// A session caught up (or not) and ready to resume.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Synced {
    pub plan: ResumePlan,
    pub caught: Caught,
    /// The head it resumes, when the session's heads are known.
    pub head: Option<Head>,
    /// The session's other heads, when it went on separately on several hosts: the branches not
    /// resumed.
    pub others: Vec<Head>,
}

impl Synced {
    /// Nothing to write, or nothing written: `plan` resumes the copy here as it is.
    pub fn up_to_date(plan: ResumePlan) -> Self {
        Self {
            plan,
            caught: Caught::UpToDate,
            head: None,
            others: Vec::new(),
        }
    }

    /// Whether the picker should stay open, saying why, rather than resume: nothing could be
    /// written, and resuming the copy here as it is should be a choice.
    pub fn holds(&self) -> bool {
        matches!(self.caught, Caught::Kept(Kept::Live { .. } | Kept::Failed(_)))
    }

    /// The status line, `None` when there's nothing to say. `host` names a head's host the way
    /// [`host_label`] does.
    pub fn status(&self, harness: HarnessKind, host: &dyn Fn(&Head) -> String) -> Option<String> {
        let from = self.head.as_ref().map(host);
        let branch = from.as_deref().map_or_else(|| "the branch".to_owned(), branch_name);
        Some(match &self.caught {
            Caught::UpToDate => return None,
            Caught::Restored { rows, note } => {
                let mut status = format!("restored {} from sync", messages(*rows));
                if !self.others.is_empty() {
                    status.push_str(&format!(", {branch}"));
                }
                if let Some(note) = note {
                    status.push_str(&format!("; {note}"));
                }
                status
            }
            Caught::FastForwarded { rows } => match from.as_deref() {
                Some(host) if host != THIS_MACHINE => {
                    format!("caught up {} from {host}", messages(*rows))
                }
                _ => format!("caught up {} from sync", messages(*rows)),
            },
            Caught::Switched { rows: 0 } => format!("switched to {branch}"),
            Caught::Switched { rows } => {
                format!("switched to {branch}: {} added beside this machine's", messages(*rows))
            }
            Caught::Kept(Kept::Live { .. }) => format!(
                "{} is running this session here — close it to catch up",
                harness_label(harness)
            ),
            Caught::Kept(Kept::Unsynced) => {
                "this machine's copy has messages sync hasn't got: resuming it as it is".to_owned()
            }
            Caught::Kept(Kept::Failed(why)) => format!("couldn't catch up: {why}"),
            Caught::Forked { why, flattened } => {
                let mut status = format!(
                    "{} can't take {branch} in place ({why}): continuing it as a new linked \
                     session",
                    harness_label(harness)
                );
                let summary = flattened.summary();
                if !summary.is_empty() {
                    status.push_str(&format!(", {summary}"));
                }
                status
            }
        })
    }

    /// The note for `atuin ai resume <id>` about the branches it didn't resume, `None` for a
    /// session that went one way.
    pub fn other_branches(
        &self,
        now: OffsetDateTime,
        host: &dyn Fn(&Head) -> String,
    ) -> Option<String> {
        if self.others.is_empty() {
            return None;
        }
        let this = self.head.as_ref().map(|h| branch_name(&host(h)))?;
        let others: Vec<String> = self.others.iter().map(|h| describe(h, now, host)).collect();
        Some(format!(
            "this session went on separately on several machines; resuming {this} (the latest). \
             Not resumed: {}",
            others.join("; ")
        ))
    }
}

/// How a head's host is named when it is this one.
pub const THIS_MACHINE: &str = "this machine";

/// A head's host as the picker names it: `this machine`, or `@name` (`host_name` says which
/// name, by id in simple form; unknown hosts show a short id).
pub fn host_label(head: &Head, here: &str, host_name: &dyn Fn(&str) -> Option<String>) -> String {
    let Some(host) = head.host else {
        // Rows from before hosts were recorded: this host's.
        return THIS_MACHINE.to_owned();
    };
    let id = host.0.as_simple().to_string();
    if id == here {
        return THIS_MACHINE.to_owned();
    }
    let name = host_name(&id).unwrap_or_else(|| id.chars().take(8).collect());
    format!("@{}", super::render::short_host(&name))
}

/// `@MacBook-Pro-3's branch`, `this machine's branch`.
pub fn branch_name(host: &str) -> String {
    format!("{host}'s branch")
}

/// `136 messages`, `1 message`.
fn messages(n: usize) -> String {
    match n {
        1 => "1 message".to_owned(),
        n => format!("{n} messages"),
    }
}

/// A branch in a line: `@MacBook-Pro-3 · 2h · 136 msgs`.
pub fn describe(head: &Head, now: OffsetDateTime, host: &dyn Fn(&Head) -> String) -> String {
    let when = super::clock::When::of(now, head.last_at, time::UtcOffset::UTC);
    let when = match when {
        super::clock::When::At(at) if at.starts_with("yest") => "yest".to_owned(),
        when => when.short().to_owned(),
    };
    format!("{} · {when} · {} msgs", host(head), head.rows)
}

/// The head `row` resumes: `picked`, when it is one of its heads, else the newest.
pub fn chosen_head<'a>(row: &'a SessionRow, picked: Option<&str>) -> Option<&'a Head> {
    picked
        .and_then(|p| row.heads.iter().find(|h| h.source_id.as_ref() == p))
        .or_else(|| row.heads.first())
}

/// Whether resuming `head` here would branch a session another host may still be working on: its
/// newest row is another host's, from the last [`LIVE_ELSEWHERE`]. Returns how long ago.
pub fn live_elsewhere(head: &Head, here: &str, now: OffsetDateTime) -> Option<Duration> {
    let host = head.host?.0.as_simple().to_string();
    let ago = now - head.last_at;
    (host != here && ago < LIVE_ELSEWHERE).then_some(ago.max(Duration::ZERO))
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::path::PathBuf;

    use atuin_common::harnesstools::sync::{LocalTip, Stamp};
    use rstest::rstest;

    use super::*;

    fn row(id: &str, parent: Option<&str>, role: Role, text: &str) -> RehydrateMessage {
        RehydrateMessage {
            source_id: id.to_owned(),
            parent_source_id: parent.map(str::to_owned),
            timestamp: OffsetDateTime::UNIX_EPOCH,
            role,
            content: vec![Content::Text(text.to_owned())],
            model: None,
            usage: None,
            stop_reason: None,
            turn_id: None,
            cwd: None,
            git_branch: None,
        }
    }

    /// `ids` as a chain, alternating prompts and replies.
    fn chain(ids: &[&str], from: Option<&str>) -> Vec<RehydrateMessage> {
        let mut parent = from.map(str::to_owned);
        ids.iter()
            .enumerate()
            .map(|(n, id)| {
                let role = if n % 2 == 0 {
                    Role::User
                } else {
                    Role::Assistant
                };
                let r = row(id, parent.as_deref(), role, "text");
                parent = Some((*id).to_owned());
                r
            })
            .collect()
    }

    fn tip(known: &[&str], at: Option<&str>) -> LocalTip {
        LocalTip {
            native_path: PathBuf::from("/t.jsonl"),
            known_source_ids: known.iter().map(|s| (*s).to_owned()).collect::<HashSet<_>>(),
            tip_source_id: at.map(str::to_owned),
            stamp: Stamp::of(b""),
        }
    }

    fn ids(step: &Step) -> Vec<&str> {
        match step {
            Step::FastForward(rows) | Step::Branch(rows) => {
                rows.iter().map(|r| r.source_id.as_str()).collect()
            }
            _ => Vec::new(),
        }
    }

    #[rstest]
    fn at_the_head_resumes_as_it_is() {
        let path = chain(&["a", "b", "c"], None);
        let step = classify(
            &tip(&["a", "b", "c"], Some("c")),
            "c",
            &path,
            None,
            false,
            HarnessKind::ClaudeCode,
        );
        assert!(matches!(step, Step::AsIs), "{step:?}");
    }

    #[rstest]
    fn behind_on_the_branch_fast_forwards() {
        let path = chain(&["a", "b", "c", "d"], None);
        let step = classify(
            &tip(&["a", "b"], Some("b")),
            "d",
            &path,
            None,
            false,
            HarnessKind::ClaudeCode,
        );
        assert!(matches!(step, Step::FastForward(_)));
        assert_eq!(ids(&step), ["c", "d"]);
    }

    /// A copy written from sync holds no line for the rows its writer merged into the line
    /// before (calls captured without their input, folded into their turn's text): they aren't
    /// caught up again.
    #[rstest]
    fn rows_merged_into_the_last_line_held_are_not_caught_up_again() {
        use atuin_common::harnesstools::session::ToolUse;

        let mut path = chain(&["a", "b"], None);
        let mut call = row("c", Some("b"), Role::Assistant, "");
        call.content = vec![Content::ToolUse(ToolUse {
            id: "t1".to_owned().into(),
            name: "Bash".to_owned(),
            input: serde_json::Value::Null,
        })];
        path.push(call);
        path.push(row("d", Some("c"), Role::Assistant, "done"));
        path.push(row("e", Some("d"), Role::User, "next"));
        path.push(row("f", Some("e"), Role::Assistant, "ok"));
        // `c` and `d` were merged into `b`'s line.
        let step = classify(
            &tip(&["a", "b"], Some("b")),
            "f",
            &path,
            None,
            false,
            HarnessKind::ClaudeCode,
        );
        assert_eq!(ids(&step), ["e", "f"]);
        // `e` hung from `d`, merged into `b`'s line: it hangs from that.
        let Step::FastForward(rows) = step else {
            panic!("{step:?}");
        };
        assert_eq!(rows[0].parent_source_id.as_deref(), Some("b"));
    }

    #[rstest]
    fn an_empty_transcript_takes_the_whole_branch() {
        let path = chain(&["a", "b"], None);
        assert_eq!(
            ids(&classify(&tip(&[], None), "b", &path, None, false, HarnessKind::ClaudeCode)),
            ["a", "b"]
        );
    }

    #[rstest]
    fn another_branch_gets_the_head_beside_it() {
        // a-b shared; this machine went on with x-y, the other host with c-d.
        let path = chain(&["a", "b", "c", "d"], None);
        let mut mine = chain(&["a", "b"], None);
        mine.extend(chain(&["x", "y"], Some("b")));
        let step = classify(
            &tip(&["a", "b", "x", "y"], Some("y")),
            "d",
            &path,
            Some(&mine),
            true,
            HarnessKind::ClaudeCode,
        );
        assert!(matches!(step, Step::Branch(_)));
        assert_eq!(ids(&step), ["c", "d"]);
    }

    #[rstest]
    fn a_branch_held_here_is_switched_to() {
        let path = chain(&["a", "b", "c", "d"], None);
        let mut mine = chain(&["a", "b"], None);
        mine.extend(chain(&["x"], Some("b")));
        let known = ["a", "b", "c", "d", "x"];
        let step = classify(
            &tip(&known, Some("x")),
            "d",
            &path,
            Some(&mine),
            true,
            HarnessKind::ClaudeCode,
        );
        assert!(matches!(step, Step::Switch), "{step:?}");
        // A rewind here, back into the head's branch: switched only when picked.
        let step =
            classify(&tip(&known, Some("b")), "d", &path, None, true, HarnessKind::ClaudeCode);
        assert!(matches!(step, Step::Switch), "{step:?}");
        let step =
            classify(&tip(&known, Some("b")), "d", &path, None, false, HarnessKind::ClaudeCode);
        assert!(matches!(step, Step::AsIs), "{step:?}");
    }

    #[rstest]
    fn a_copy_past_the_head_or_with_unsynced_rows_is_kept() {
        let path = chain(&["a", "b"], None);
        // Sync knows the tip, which descends from the head.
        let ahead = chain(&["a", "b", "c"], None);
        let step = classify(
            &tip(&["a", "b", "c"], Some("c")),
            "b",
            &path,
            Some(&ahead),
            false,
            HarnessKind::ClaudeCode,
        );
        assert!(matches!(step, Step::Ahead), "{step:?}");
        // Sync doesn't know the tip: past the head, or a branch of its own.
        let step = classify(
            &tip(&["a", "b", "z"], Some("z")),
            "b",
            &path,
            Some(&[]),
            false,
            HarnessKind::ClaudeCode,
        );
        assert!(matches!(step, Step::Ahead), "{step:?}");
        let step = classify(
            &tip(&["a", "z"], Some("z")),
            "b",
            &path,
            Some(&[]),
            false,
            HarnessKind::ClaudeCode,
        );
        assert!(matches!(step, Step::Unsynced), "{step:?}");
    }

    #[rstest]
    fn a_twig_off_the_branch_is_no_branch() {
        // The copy stopped on a tool result hanging off `b`; the head went on from `b`.
        let path = chain(&["a", "b", "c", "d"], None);
        let mut twig = chain(&["a", "b"], None);
        twig.push(row("t", Some("b"), Role::Tool, "ok"));
        let step = classify(
            &tip(&["a", "b", "t"], Some("t")),
            "d",
            &path,
            Some(&twig),
            false,
            HarnessKind::ClaudeCode,
        );
        assert_eq!(ids(&step), ["c", "d"]);
        assert!(matches!(step, Step::FastForward(_)));
    }

    fn head(host: u128, minutes_ago: i64, rows: u64) -> Head {
        Head {
            source_id: format!("h{host}").into(),
            host: Some(atuin_domain::record::HostId(uuid::Uuid::from_u128(host))),
            last_at: now() - Duration::minutes(minutes_ago),
            rows,
        }
    }

    fn now() -> OffsetDateTime {
        time::macros::datetime!(2026-09-27 15:00:00 UTC)
    }

    fn here() -> String {
        uuid::Uuid::from_u128(1).as_simple().to_string()
    }

    #[rstest]
    #[case::another_host_just_now(2, 2, true)]
    #[case::another_host_a_while_ago(2, 6, false)]
    #[case::this_host(1, 1, false)]
    fn a_recent_head_from_another_host_is_live_elsewhere(
        #[case] host: u128,
        #[case] minutes: i64,
        #[case] live: bool,
    ) {
        assert_eq!(live_elsewhere(&head(host, minutes, 3), &here(), now()).is_some(), live);
    }

    #[rstest]
    fn statuses_name_the_host() {
        let names = |id: &str| {
            (id == uuid::Uuid::from_u128(2).as_simple().to_string())
                .then(|| "MacBook-Pro-3.local".to_owned())
        };
        let label = |h: &Head| host_label(h, &here(), &names);
        let plan = ResumePlan {
            program: "claude".to_owned(),
            args: Vec::new(),
            cwd: None,
            cwd_requirement: atuin_common::harnesstools::resume::CwdRequirement::Preferred,
            native_path: None,
        };
        let mac = head(2, 120, 136);
        let mine = head(1, 60 * 20, 47);
        let synced = |caught| Synced {
            plan: plan.clone(),
            caught,
            head: Some(mac.clone()),
            others: vec![mine.clone()],
        };
        let status = |caught| synced(caught).status(HarnessKind::ClaudeCode, &label).unwrap();
        assert_eq!(
            status(Caught::FastForwarded { rows: 136 }),
            "caught up 136 messages from @MacBook-Pro-3"
        );
        assert_eq!(
            status(Caught::Switched { rows: 136 }),
            "switched to @MacBook-Pro-3's branch: 136 messages added beside this machine's"
        );
        assert_eq!(
            status(Caught::Kept(Kept::Live { pid: Some(7) })),
            "Claude Code is running this session here — close it to catch up"
        );
        assert!(
            status(Caught::Forked {
                why: "opencode keeps a session as one line".to_owned(),
                flattened: Flattened::default()
            })
            .contains("can't take @MacBook-Pro-3's branch in place")
        );
        assert_eq!(synced(Caught::UpToDate).status(HarnessKind::ClaudeCode, &label), None);
        assert_eq!(
            synced(Caught::UpToDate).other_branches(now(), &label).unwrap(),
            "this session went on separately on several machines; resuming @MacBook-Pro-3's \
             branch (the latest). Not resumed: this machine · yest · 47 msgs"
        );
        assert_eq!(describe(&mac, now(), &label), "@MacBook-Pro-3 · 2h · 136 msgs");
    }
}
