//! Catching this machine's copy of a session up with sync before resuming it in its own agent.
//!
//! A session keeps one id on every machine: each machine's transcript is a line of it, and the
//! synced rows hold every line ([`Analysis`]). Resuming a session here brings its copy to a head
//! (the newest, unless `--branch` picked another), and never merges:
//!
//! - no copy here: it is restored from sync ([`Resumer::restore`]); along the head's branch
//!   ([`branch_rows`]) when one was named or the session diverged, else from every row, as before;
//! - the copy is at the head, or went on past it ([`Step::AsIs`], [`Step::Ahead`]): resumed as it
//!   is;
//! - the copy is behind on the head's line ([`Step::FastForward`]), and no agent here has it open:
//!   the rows it lacks are appended, and it resumes in place;
//! - anything else ([`Step::Choice`]): nothing is written, and the user chooses between resuming
//!   the copy as it is and forking from a head ([`Held`]).
//!
//! [`Resumer::restore`]: super::resumer::Resumer::restore

use atuin_client::ai_session::{
    Analysis, FastForward, HarnessKind, HarnessSession, Head, Message, SourceId,
};
use atuin_common::harnesstools::rehydrate::RehydrateMessage;
use atuin_common::harnesstools::session::{Content, is_substantive};
use atuin_common::harnesstools::sync::LocalTip;
use time::OffsetDateTime;

use super::clock::When;
use super::resumer::{ForkFrom, NotResumable, ResumePlan};
use super::source::{SessionSource, harness_label};

/// How bringing this machine's copy of a session to a head goes ([`classify`]).
#[derive(Debug, Clone)]
pub enum Step {
    /// The copy is at the head.
    AsIs,
    /// The copy holds the head and went on past it here.
    Ahead,
    /// The copy is behind on the head's line: append `rows` to it, read as `base`.
    FastForward {
        rows: Vec<RehydrateMessage>,
        base: LocalTip,
    },
    /// Nothing can be written: resuming the copy as it is, or forking, is the user's choice.
    Choice(Why),
}

/// Why catching up needs the user to choose.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Why {
    /// The copy is on another branch than the head's.
    Diverged,
    /// The copy goes on from a row sync hasn't got.
    Unsynced,
    /// An agent here has the session open, or may have.
    Live,
    /// The agent's copy couldn't take the rows (see [`SyncError`]).
    ///
    /// [`SyncError`]: atuin_common::harnesstools::sync::SyncError
    Refused(String),
}

/// How to bring the copy read as `tip` to `head`, one of `analysis`'s heads, for a session of
/// `harness`.
///
/// The copy is at the head, or ahead of it, only when the head is on the line its tip goes on
/// from: a copy holding the head that was rewound onto a line of its own is not. A tip sync
/// hasn't got may be either, which can't be told: that is a choice ([`Why::Unsynced`]).
///
/// The twig rule: a copy that stopped on a twig off the head's line (a tool result, an attachment:
/// nothing [substantive](is_substantive) past where it leaves the line, and no summary standing in
/// for the conversation, which the model is given as context) is no branch of its own, and is
/// fast-forwarded from where it leaves it, as the agent would go on from there. Only for a harness
/// whose rows name the row they follow (Claude Code, pi), where the rows appended hang from that
/// row; any other reads its transcript in order, twig and all.
pub fn classify(
    analysis: &Analysis,
    harness: HarnessKind,
    head: &SourceId,
    tip: &LocalTip,
) -> Step {
    match analysis.fast_forward(head, tip) {
        FastForward::NotBehind => return Step::AsIs,
        FastForward::Behind(rows) => {
            return Step::FastForward {
                rows,
                base: tip.clone(),
            };
        }
        FastForward::Elsewhere => {}
    }
    let Some(at) = tip.tip_source_id.as_deref() else {
        return Step::Choice(Why::Diverged);
    };
    let mine = analysis.path_to(&SourceId::from(at.to_owned()));
    if mine.is_empty() {
        // Sync hasn't got the row the copy goes on from: past the head, or a line of its own
        // (rewound here, even if it holds the head), which can't be told apart.
        return Step::Choice(Why::Unsynced);
    }
    if mine.iter().any(|m| m.source_id == *head) {
        return Step::Ahead;
    }
    let path = analysis.path_to(head);
    let shared = path.iter().zip(&mine).take_while(|(a, b)| a.source_id == b.source_id).count();
    let linked = matches!(harness, HarnessKind::ClaudeCode | HarnessKind::Pi);
    let twig = linked && !mine[shared..].iter().any(|m| carries_conversation(m));
    let rest = &path[shared..];
    let holds_none = !rest.iter().any(|m| tip.known_source_ids.contains(m.source_id.as_ref()));
    if twig && holds_none && shared > 0 {
        let mut base = tip.clone();
        base.tip_source_id = Some(path[shared - 1].source_id.to_string());
        let rows = rest.iter().map(|&m| m.clone().into()).collect();
        return Step::FastForward { rows, base };
    }
    Step::Choice(Why::Diverged)
}

/// Whether going on from before `row` would lose conversation: it is
/// [substantive](is_substantive), or a summary standing in for earlier conversation (a
/// compaction), which the model is given as context. Only for the twig rule: a summary makes no
/// branch of its own as far as [`Analysis`] goes (see [`is_substantive`]).
fn carries_conversation(row: &Message) -> bool {
    is_substantive(&row.role, &row.content)
        || row.content.iter().any(|c| matches!(c, Content::Summary(_)))
}

/// How many messages `rows` hold: prompts and assistant text, as a [`Head`] counts them.
pub fn message_count(rows: &[RehydrateMessage]) -> usize {
    rows.iter().filter(|m| is_substantive(&m.role, &m.content)).count()
}

/// A head to fork from, as the chooser offers it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Branch {
    pub head: Head,
    /// Its host, as [`host_label`] names it.
    pub host: String,
    /// What `--branch` names it by ([`branch_selector`]).
    pub selector: String,
    /// Its messages this machine's copy hasn't got: since they split.
    pub ahead: usize,
}

impl Branch {
    /// The chooser's line: `fork @3f9a12bc's · +11 since they split · 20m ago`.
    pub fn line(&self, now: OffsetDateTime) -> String {
        let when = When::of(now, self.head.last_at, time::UtcOffset::UTC).phrase();
        match self.ahead {
            0 => format!("fork {}'s · {when}", self.host),
            n => format!("fork {}'s · +{n} since they split · {when}", self.host),
        }
    }
}

/// `analysis`'s heads, newest first, as [`Branch`]es against the copy read as `tip`; `here` is
/// this host's id.
pub fn branches(analysis: &Analysis, tip: Option<&LocalTip>, here: &str) -> Vec<Branch> {
    let heads = analysis.heads();
    heads
        .iter()
        .map(|head| {
            let path = analysis.path_to(&head.source_id);
            let ahead = path
                .iter()
                .filter(|m| is_substantive(&m.role, &m.content))
                .filter(|m| tip.is_none_or(|t| !t.known_source_ids.contains(m.source_id.as_ref())))
                .count();
            Branch {
                head: head.clone(),
                host: host_label(head, here),
                selector: branch_selector(heads, head),
                ahead,
            }
        })
        .collect()
}

/// Catching up needs a choice: nothing was written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Held {
    pub why: Why,
    pub harness: HarnessKind,
    /// Resumes this machine's copy as it is.
    pub plan: ResumePlan,
    /// The session's heads to fork from, newest first.
    pub branches: Vec<Branch>,
    /// The one catching up went to, in `branches`.
    pub chosen: usize,
}

impl Held {
    /// The status line.
    pub fn status(&self) -> String {
        match &self.why {
            Why::Live => format!("{} is running this session here", harness_label(self.harness)),
            Why::Unsynced => "this copy has messages sync hasn't got".to_owned(),
            Why::Diverged => match self.branches.get(self.chosen) {
                Some(branch) => format!("this copy went another way than {}'s", branch.host),
                None => "this copy went another way".to_owned(),
            },
            Why::Refused(why) => format!("couldn't catch up: {why}"),
        }
    }
}

/// What catching up did ([`super::resumer::Resumer::catch_up`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CatchUp {
    /// Resume with `plan`; `status` says what was written, if anything.
    Ready {
        plan: ResumePlan,
        status: Option<String>,
    },
    Choice(Box<Held>),
}

/// `caught up: 2 messages from @3f9a12bc`.
pub fn caught_up(messages: usize, host: &str) -> String {
    let s = if messages == 1 {
        ""
    } else {
        "s"
    };
    format!("caught up: {messages} message{s} from {host}")
}

/// How a head's host is named when it is this one.
pub const THIS_MACHINE: &str = "this machine";

/// A head's host as the picker names it: `this machine`, or `@3f9a12bc` for another, by a short
/// form of its id (see [`super::source::host_label`]).
pub fn host_label(head: &Head, here: &str) -> String {
    // Rows from before hosts were recorded are this host's.
    head.host.map_or_else(
        || THIS_MACHINE.to_owned(),
        |host| super::source::host_label(&host.0.as_simple().to_string(), here),
    )
}

/// The fewest characters a [`branch_selector`] has, so it stays unique as a session gains heads.
const SELECTOR_MIN: usize = 8;

/// How `--branch` names `head` among `heads` for good: the shortest start of its source id,
/// [`SELECTOR_MIN`] characters or more, that no other head's id starts with; else the whole id.
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

/// A head in a line: `@3f9a12bc · 2h · 136 msgs`.
pub fn describe(head: &Head, now: OffsetDateTime, here: &str) -> String {
    let when = When::of(now, head.last_at, time::UtcOffset::UTC);
    format!("{} · {} · {} msgs", host_label(head, here), when.short(), head.messages)
}

/// The head `selector` names among `heads` (`--branch`): `this` for this machine's, `@3f9a12bc`
/// for another host's (by the end of its id, or all of it), else the start of its source id (see
/// [`branch_selector`]). Anything naming no single head is an error listing them.
pub fn pick_branch<'a>(
    heads: &'a [Head],
    selector: &str,
    now: OffsetDateTime,
    here: &str,
) -> Result<&'a Head, String> {
    if heads.is_empty() {
        return Err("its branches aren't known yet: the daemon hasn't synced its messages".into());
    }
    let selector = selector.trim();
    let host = |h: &Head| host_label(h, here);
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
        let lines = heads.iter().enumerate().map(|(n, h)| {
            let latest = if n == 0 {
                " (latest)"
            } else {
                ""
            };
            format!("\n  {}  {}{latest}", branch_selector(heads, h), describe(h, now, here))
        });
        lines.collect::<String>()
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

/// What forking `session` from `head` starts from: the rows of that head's branch
/// ([`branch_rows`]). Without one, a session that went on separately on several machines forks
/// from its newest head, and any other from every row synced of it ([`ForkFrom::default`]).
pub async fn fork_from(
    source: &dyn SessionSource,
    session: &HarnessSession,
    head: Option<&SourceId>,
) -> Result<ForkFrom, NotResumable> {
    let rows = branch_rows(source, session, head)
        .await
        .map_err(|e| NotResumable::Fork(format!("{e:#}")))?;
    Ok(ForkFrom { rows, tip: None })
}

/// The rows a transcript of `session` written out along `head` holds (see
/// [`Analysis::rows_for`]): a head named (`--branch`, or a fork line picked) is followed; without
/// one, a session that went on separately on several machines follows its newest head, so no
/// branch's messages land in another's. `None` for any other session (or one whose branches
/// aren't known): it is written out from every row synced of it, as it always was, rows off every
/// branch included.
pub async fn branch_rows(
    source: &dyn SessionSource,
    session: &HarnessSession,
    head: Option<&SourceId>,
) -> eyre::Result<Option<Vec<RehydrateMessage>>> {
    let Some(analysis) = source.analyse(session).await? else {
        return Ok(None);
    };
    let newest = analysis.diverged().then(|| analysis.heads().first()).flatten();
    let Some(head) = head.or(newest.map(|h| &h.source_id)) else {
        return Ok(None);
    };
    Ok(Some(analysis.rows_for(head).into_iter().map(|m| m.clone().into()).collect()))
}

#[cfg(test)]
mod tests;
