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

use atuin_client::ai_session::{HarnessKind, Head};
use atuin_common::harnesstools::continuation::Flattened;
use atuin_common::harnesstools::rehydrate::RehydrateMessage;
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
/// decides which forks are branches; tool calls, attachments and compactions aren't). These are
/// what the picker counts as messages ([`message_count`]).
fn substantive(row: &RehydrateMessage) -> bool {
    match row.role {
        Role::User => true,
        Role::Assistant => {
            row.content.iter().any(|c| matches!(c, Content::Text(t) if !t.trim().is_empty()))
        }
        _ => false,
    }
}

/// How many messages `rows` hold: user prompts and assistant text, not tool calls, their results
/// or the harness's own lines. What a branch is counted in ([`Head::messages`], as the sidecar
/// counts it), and what catching up says it added.
pub fn message_count(rows: &[RehydrateMessage]) -> usize {
    rows.iter().filter(|row| substantive(row)).count()
}

/// The rows of `path` after the last one `tip` holds: all of them when it holds none.
///
/// What `tip` holds counts the rows a writer merged into its lines (see
/// [`LocalTip::merged`]), so the calls a restore folded into notes are never caught up again,
/// and the harness writes a row hanging from one of them from the line it went into. The first
/// row names the one before it as where it goes on from when it names nothing itself: Codex's
/// rows name no parent, and it places a branch by it.
fn missing(path: &[RehydrateMessage], tip: &LocalTip) -> Vec<RehydrateMessage> {
    let Some(last) = path.iter().rposition(|m| tip.known_source_ids.contains(&m.source_id)) else {
        return path.to_vec();
    };
    let mut rows = path[last + 1..].to_vec();
    if let Some(first) = rows.first_mut()
        && first.parent_source_id.is_none()
    {
        first.parent_source_id = Some(path[last].source_id.clone());
    }
    rows
}

/// How to bring the copy read as `tip` to `head`, whose branch (root to head) is `path`.
///
/// `tip_path` is the branch the copy's own tip is on, as sync has it (empty when sync doesn't
/// hold that row); only needed when the tip isn't on `path`. `picked`: the user picked this head
/// among several, so a copy holding it is switched to it even when it continues from an earlier
/// row of it.
pub fn classify(
    tip: &LocalTip,
    head: &str,
    path: &[RehydrateMessage],
    tip_path: Option<&[RehydrateMessage]>,
    picked: bool,
) -> Step {
    let missing = missing(path, tip);
    let Some(at) = tip.tip_source_id.as_deref() else {
        // A transcript the harness would start empty.
        return if missing.is_empty() {
            Step::AsIs
        } else {
            Step::FastForward(missing)
        };
    };
    // The head itself, or merged into the line the copy continues from.
    if at == head || tip.merged.get(head).is_some_and(|into| into == at) {
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
    Current,
    /// There was no copy here: it was written out from sync, holding `messages` messages (see
    /// [`message_count`]). `note` says where it resumes when that isn't where it ran.
    Restored {
        messages: usize,
        note: Option<String>,
    },
    /// `messages` messages appended to a copy that was behind (besides the tool calls and the
    /// like between them).
    FastForwarded {
        messages: usize,
    },
    /// The head's branch made the one the session continues from, in the same session:
    /// `messages` messages appended beside this machine's (none when it already held them).
    Switched {
        messages: usize,
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
            caught: Caught::Current,
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
            Caught::Current => return None,
            Caught::Restored { messages: n, note } => {
                let mut status = format!("restored {} from sync", messages(*n));
                if !self.others.is_empty() {
                    status.push_str(&format!(", {branch}"));
                }
                if let Some(note) = note {
                    status.push_str(&format!("; {note}"));
                }
                status
            }
            Caught::FastForwarded { messages: n } => {
                // Only tool calls and the like: nothing to count.
                let what = match n {
                    0 => String::new(),
                    n => format!("{} ", messages(*n)),
                };
                match from.as_deref() {
                    Some(host) if host != THIS_MACHINE => format!("caught up {what}from {host}"),
                    _ => format!("caught up {what}from sync"),
                }
            }
            Caught::Switched { messages: 0 } => format!("switched to {branch}"),
            Caught::Switched { messages: n } => {
                format!("switched to {branch}: {} added beside this machine's", messages(*n))
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
        let head = self.head.as_ref()?;
        let this = branch_name(&host(head));
        // `--branch` may have picked one older than the others.
        let why = if self.others.iter().all(|o| o.last_at <= head.last_at) {
            "the latest"
        } else {
            "as picked"
        };
        let others: Vec<String> = self.others.iter().map(|h| describe(h, now, host)).collect();
        Some(format!(
            "this session went on separately on several machines; resuming {this} ({why}). Not \
             resumed: {}",
            others.join("; ")
        ))
    }
}

/// How a head's host is named when it is this one.
pub const THIS_MACHINE: &str = "this machine";

/// A head's host as the picker names it (see [`super::source::host_label`]): `this machine`, or
/// `@3f9a12bc` for another host, by a short form of its id.
pub fn host_label(head: &Head, here: &str) -> String {
    // Rows from before hosts were recorded are this host's.
    head.host.map_or_else(
        || THIS_MACHINE.to_owned(),
        |host| super::source::host_label(&host.0.as_simple().to_string(), here),
    )
}

/// The fewest characters a [`branch_selector`] has, so it stays unique as a session gains
/// branches.
const SELECTOR_MIN: usize = 8;

/// How `atuin ai resume --branch` names `head` among `heads` for good (what the picker's ctrl-y
/// copies): the shortest start of its source id, [`SELECTOR_MIN`] characters or more, that no
/// other head's id starts with; the whole id when there is none. Unlike its host, it names the
/// same branch however the session goes on.
pub fn branch_selector(heads: &[Head], head: &Head) -> String {
    let id = head.source_id.as_ref();
    let others: Vec<&str> =
        heads.iter().map(|h| h.source_id.as_ref()).filter(|other| *other != id).collect();
    let ends = id.char_indices().map(|(at, _)| at).skip(SELECTOR_MIN).chain([id.len()]);
    for end in ends {
        let prefix = &id[..end];
        if others.iter().all(|other| !other.starts_with(prefix)) {
            return prefix.to_owned();
        }
    }
    id.to_owned()
}

/// The head `selector` names among `heads` (`atuin ai resume --branch`): `this` for this
/// machine's, `@3f9a12bc` for another host's (as [`host_label`] names it, which `host` does, or
/// by its full id), else the start of its source id (see [`branch_selector`]; a head's whole id
/// is its own even when longer ids start with it). Anything that names no single head is an
/// error listing the branches.
pub fn pick_branch<'a>(
    heads: &'a [Head],
    selector: &str,
    now: OffsetDateTime,
    host: &dyn Fn(&Head) -> String,
) -> Result<&'a Head, String> {
    if heads.is_empty() {
        return Err("its branches aren't known yet: the daemon works them out".to_owned());
    }
    let selector = selector.trim();
    let found: Vec<&Head> = if selector == "this" {
        heads.iter().filter(|h| host(h) == THIS_MACHINE).collect()
    } else if let Some(id) = selector.strip_prefix('@') {
        let short = format!("@{}", super::source::short_host_id(&super::simple_host_id(id)));
        heads.iter().filter(|h| host(h).eq_ignore_ascii_case(&short)).collect()
    } else if let Some(exact) = heads.iter().find(|h| h.source_id.as_ref() == selector) {
        vec![exact]
    } else if selector.is_empty() {
        Vec::new()
    } else {
        heads.iter().filter(|h| h.source_id.as_ref().starts_with(selector)).collect()
    };
    let listing = || {
        heads
            .iter()
            .enumerate()
            .map(|(n, h)| {
                let latest = if n == 0 {
                    " (latest)"
                } else {
                    ""
                };
                format!("\n  {}  {}{latest}", branch_selector(heads, h), describe(h, now, host))
            })
            .collect::<String>()
    };
    match found.as_slice() {
        [head] => Ok(head),
        [] => Err(format!("no branch is {selector:?}; its branches:{}", listing())),
        several => Err(format!(
            "{selector:?} names {} branches; name one by id:{}",
            several.len(),
            listing()
        )),
    }
}

/// `@3f9a12bc's branch`, `this machine's branch`.
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

/// A branch in a line: `@3f9a12bc · 2h · 136 msgs`.
pub fn describe(head: &Head, now: OffsetDateTime, host: &dyn Fn(&Head) -> String) -> String {
    let when = super::clock::When::of(now, head.last_at, time::UtcOffset::UTC);
    let when = match when {
        super::clock::When::At(at) if at.starts_with("yest") => "yest".to_owned(),
        when => when.short().to_owned(),
    };
    format!("{} · {when} · {} msgs", host(head), head.messages)
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
            seq: None,
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
            merged: std::collections::HashMap::new(),
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
        let step = classify(&tip(&["a", "b", "c"], Some("c")), "c", &path, None, false);
        assert!(matches!(step, Step::AsIs), "{step:?}");
    }

    #[rstest]
    fn behind_on_the_branch_fast_forwards() {
        let path = chain(&["a", "b", "c", "d"], None);
        let step = classify(&tip(&["a", "b"], Some("b")), "d", &path, None, false);
        assert!(matches!(step, Step::FastForward(_)));
        assert_eq!(ids(&step), ["c", "d"]);
    }

    /// A copy written from sync holds no line for the rows its writer merged into another (calls
    /// captured without their input, folded into their turn's text), but says it holds them:
    /// they aren't caught up again, a head merged into the line it continues from is where it
    /// is, and the rows after them still hang from them (the harness writes them hanging from the
    /// line they went into).
    #[rstest]
    fn rows_merged_into_a_line_are_held() {
        let path = chain(&["a", "b", "c", "d", "e", "f"], None);
        let mut copy = tip(&["a", "b", "c", "d"], Some("b"));
        copy.merged = [("c", "b"), ("d", "b")]
            .into_iter()
            .map(|(row, into)| (row.to_owned(), into.to_owned()))
            .collect();
        let step = classify(&copy, "f", &path, None, false);
        assert_eq!(ids(&step), ["e", "f"]);
        let Step::FastForward(rows) = step else {
            panic!("{step:?}");
        };
        assert_eq!(rows[0].parent_source_id.as_deref(), Some("d"));
        let step = classify(&copy, "d", &path[..4], None, false);
        assert!(matches!(step, Step::AsIs), "{step:?}");
    }

    /// Codex's rows name no parent: the first row caught up names the last one the copy holds,
    /// which is where Codex places a branch.
    #[rstest]
    fn the_first_row_names_where_it_goes_on_from() {
        let mut path = chain(&["a", "b", "c"], None);
        for row in &mut path {
            row.parent_source_id = None;
        }
        let step = classify(&tip(&["a", "b"], Some("b")), "c", &path, None, false);
        let Step::FastForward(rows) = step else {
            panic!("{step:?}");
        };
        assert_eq!(rows[0].parent_source_id.as_deref(), Some("b"));
    }

    /// Messages are prompts and assistant text: not calls, their results, or the harness's own
    /// lines, which the branch labels don't count either.
    #[rstest]
    fn messages_are_prompts_and_replies() {
        use atuin_common::harnesstools::session::ToolUse;

        let mut rows = chain(&["a", "b", "c", "d"], None);
        rows[1].content = vec![Content::ToolUse(ToolUse {
            id: "t1".to_owned().into(),
            name: "Bash".to_owned(),
            input: serde_json::Value::Null,
        })];
        rows.push(row("t", Some("d"), Role::Tool, "ok"));
        rows.push(row("m", Some("t"), Role::System, "a hook said so"));
        assert_eq!(message_count(&rows), 3);
    }

    #[rstest]
    fn an_empty_transcript_takes_the_whole_branch() {
        let path = chain(&["a", "b"], None);
        assert_eq!(ids(&classify(&tip(&[], None), "b", &path, None, false)), ["a", "b"]);
    }

    #[rstest]
    fn another_branch_gets_the_head_beside_it() {
        // a-b shared; this machine went on with x-y, the other host with c-d.
        let path = chain(&["a", "b", "c", "d"], None);
        let mut mine = chain(&["a", "b"], None);
        mine.extend(chain(&["x", "y"], Some("b")));
        let step = classify(&tip(&["a", "b", "x", "y"], Some("y")), "d", &path, Some(&mine), true);
        assert!(matches!(step, Step::Branch(_)));
        assert_eq!(ids(&step), ["c", "d"]);
    }

    #[rstest]
    fn a_branch_held_here_is_switched_to() {
        let path = chain(&["a", "b", "c", "d"], None);
        let mut mine = chain(&["a", "b"], None);
        mine.extend(chain(&["x"], Some("b")));
        let known = ["a", "b", "c", "d", "x"];
        let step = classify(&tip(&known, Some("x")), "d", &path, Some(&mine), true);
        assert!(matches!(step, Step::Switch), "{step:?}");
        // A rewind here, back into the head's branch: switched only when picked.
        let step = classify(&tip(&known, Some("b")), "d", &path, None, true);
        assert!(matches!(step, Step::Switch), "{step:?}");
        let step = classify(&tip(&known, Some("b")), "d", &path, None, false);
        assert!(matches!(step, Step::AsIs), "{step:?}");
    }

    #[rstest]
    fn a_copy_past_the_head_or_with_unsynced_rows_is_kept() {
        let path = chain(&["a", "b"], None);
        // Sync knows the tip, which descends from the head.
        let ahead = chain(&["a", "b", "c"], None);
        let step = classify(&tip(&["a", "b", "c"], Some("c")), "b", &path, Some(&ahead), false);
        assert!(matches!(step, Step::Ahead), "{step:?}");
        // Sync doesn't know the tip: past the head, or a branch of its own.
        let step = classify(&tip(&["a", "b", "z"], Some("z")), "b", &path, Some(&[]), false);
        assert!(matches!(step, Step::Ahead), "{step:?}");
        let step = classify(&tip(&["a", "z"], Some("z")), "b", &path, Some(&[]), false);
        assert!(matches!(step, Step::Unsynced), "{step:?}");
    }

    #[rstest]
    fn a_twig_off_the_branch_is_no_branch() {
        // The copy stopped on a tool result hanging off `b`; the head went on from `b`.
        let path = chain(&["a", "b", "c", "d"], None);
        let mut twig = chain(&["a", "b"], None);
        twig.push(row("t", Some("b"), Role::Tool, "ok"));
        let step = classify(&tip(&["a", "b", "t"], Some("t")), "d", &path, Some(&twig), false);
        assert_eq!(ids(&step), ["c", "d"]);
        assert!(matches!(step, Step::FastForward(_)));
    }

    fn head(host: u128, minutes_ago: i64, messages: u64) -> Head {
        Head {
            source_id: format!("h{host}").into(),
            host: Some(atuin_domain::record::HostId(uuid::Uuid::from_u128(host))),
            last_at: now() - Duration::minutes(minutes_ago),
            rows: messages * 2,
            messages,
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
        let label = |h: &Head| host_label(h, &here());
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
            status(Caught::FastForwarded { messages: 136 }),
            "caught up 136 messages from @00000002"
        );
        assert_eq!(
            status(Caught::Switched { messages: 136 }),
            "switched to @00000002's branch: 136 messages added beside this machine's"
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
            .contains("can't take @00000002's branch in place")
        );
        assert_eq!(synced(Caught::Current).status(HarnessKind::ClaudeCode, &label), None);
        assert_eq!(
            synced(Caught::Current).other_branches(now(), &label).unwrap(),
            "this session went on separately on several machines; resuming @00000002's branch \
             (the latest). Not resumed: this machine · yest · 47 msgs"
        );
        assert_eq!(describe(&mac, now(), &label), "@00000002 · 2h · 136 msgs");
    }

    fn head_with_id(id: &str, host: u128, minutes_ago: i64, messages: u64) -> Head {
        Head {
            source_id: id.to_owned().into(),
            ..head(host, minutes_ago, messages)
        }
    }

    /// A branch's selector is the shortest start of its id that no other branch shares, eight
    /// characters at least, or the whole id; and it picks that branch back.
    #[rstest]
    #[case::uuid(&["7aaabc31-1631-4756", "0c1d2e3f-8a9b-4c5d"], "7aaabc31")]
    #[case::sharing_a_long_start(&["prt_fcf1e6fc6001aa", "prt_fcf1e6fc9002bb"], "prt_fcf1e6fc6")]
    #[case::short(&["b40", "h24"], "b40")]
    #[case::a_start_of_another(&["b4", "b40"], "b4")]
    fn a_selector_is_the_shortest_start_no_other_branch_shares(
        #[case] ids: &[&str],
        #[case] want: &str,
    ) {
        let heads: Vec<Head> = ids.iter().map(|id| head_with_id(id, 1, 0, 1)).collect();
        let selector = branch_selector(&heads, &heads[0]);
        assert_eq!(selector, want);
        let label = |h: &Head| host_label(h, &here());
        let picked = pick_branch(&heads, &selector, now(), &label).unwrap();
        assert_eq!(picked, &heads[0], "it picks the branch back");
    }

    /// `--branch` takes `this`, `@host` (its short or full id) or the start of a branch's id;
    /// anything naming no single branch is an error listing them.
    #[rstest]
    #[case::this("this", Ok("h1"))]
    #[case::host("@00000002", Ok("h2"))]
    #[case::full_host_id("@00000000-0000-0000-0000-000000000002", Ok("h2"))]
    #[case::id("h3", Ok("h3"))]
    #[case::unknown_host("@buildbox", Err("no branch is \"@buildbox\"; its branches:"))]
    #[case::unknown_id("zz", Err("no branch is \"zz\""))]
    #[case::ambiguous_id("h", Err("\"h\" names 3 branches; name one by id:"))]
    fn branches_are_picked_by_host_or_id(#[case] selector: &str, #[case] want: Result<&str, &str>) {
        let label = |h: &Head| host_label(h, &here());
        let heads = [head(2, 60, 40), head(1, 120, 24), head(3, 180, 5)];
        let picked = pick_branch(&heads, selector, now(), &label);
        match want {
            Ok(id) => assert_eq!(picked.unwrap().source_id.as_ref(), id),
            Err(start) => {
                let why = picked.unwrap_err();
                assert!(why.starts_with(start), "{why}");
                let listing = "\n  h2  @00000002 · 1h · 40 msgs (latest)\n  h1  this machine · 2h \
                               · 24 msgs\n  h3  @00000003 · 3h · 5 msgs";
                assert!(why.ends_with(listing), "{why}");
            }
        }

        // Two branches on one host: the host names neither.
        let heads = [head(2, 60, 40), head_with_id("h2b", 2, 90, 3)];
        let why = pick_branch(&heads, "@00000002", now(), &label).unwrap_err();
        assert!(why.starts_with("\"@00000002\" names 2 branches"), "{why}");
        assert!(pick_branch(&[], "this", now(), &label).is_err());
    }

    /// Resuming a branch `--branch` picked that isn't the newest says so.
    #[rstest]
    fn other_branches_say_when_the_one_resumed_was_picked() {
        let plan = ResumePlan {
            program: "claude".to_owned(),
            args: Vec::new(),
            cwd: None,
            cwd_requirement: atuin_common::harnesstools::resume::CwdRequirement::Preferred,
            native_path: None,
        };
        let synced = Synced {
            plan,
            caught: Caught::Current,
            head: Some(head(1, 120, 24)),
            others: vec![head(2, 60, 40)],
        };
        let label = |h: &Head| host_label(h, &here());
        let note = synced.other_branches(now(), &label).unwrap();
        assert!(note.contains("resuming this machine's branch (as picked)"), "{note}");
    }
}
