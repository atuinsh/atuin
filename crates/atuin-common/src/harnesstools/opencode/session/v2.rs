//! opencode 2.0's sessions: its `session_v2` table and the `session_message` projection.
//!
//! opencode 2.0 (`@opencode/cli`, the `v2` branch) writes the same `opencode.db` as 1.x, but its
//! bus no longer persists durable events (`Bus.publish` defaults to `persist: false`,
//! `packages/core/src/bus.ts`), so the `event` table the 1.x reader tails stays empty. What it
//! keeps instead is a projection: one `session_v2` row per session and one `session_message` row
//! per message (`packages/core/src/session/projector.ts`, `message-updater.ts`), whose `data` is a
//! `SessionMessage.Info` (`packages/schema/src/session-message.ts`) minus its `id` and `type`.
//!
//! # The rows
//!
//! A row is inserted by the durable event that creates it, with that event's `seq` (the session's
//! event sequence, which only grows), and some rows are then **updated in place**: an assistant
//! row is inserted when its model call starts and rewritten as the call streams, until `Step.Ended`
//! or `Step.Failed` stamps `time.completed`; a compaction row is `running` until it completes or
//! fails; a shell row is `running` until its command exits. A revert deletes every row from its
//! boundary's `seq` on. A row is delivered once, when it is settled: the consumer keeps the first
//! message of each id, so a draft delivered would stand for good. A shell row is delivered at once,
//! since what is read of it -- the command the user ran -- never changes.
//!
//! # Change detection
//!
//! Every write to a session is a durable event, and every durable event advances
//! `event_sequence.seq` of its session in the transaction that projects it (`bus.ts`, `publish`),
//! persisted or not. [`changes`] polls `PRAGMA data_version` on a connection of its own and, when
//! another connection has committed, reads `event_sequence` joined to `session_v2`: a session whose
//! `seq` moved is woken, and one never seen before is offered. The session then reads its own rows.
//!
//! # Checkpoints
//!
//! Rows do not settle in `seq` order: a prompt steered in while a call runs is inserted after the
//! call's row and is settled first, and a shell may run across many turns. A session therefore
//! delivers every settled row as it finds it, and its position -- the [`Mark`] -- is the last row
//! of the longest prefix of the session in which every row has been delivered or needs no delivery;
//! rows past it that were delivered are remembered, in memory, until the mark passes them. A
//! checkpoint names the mark, so a session resumed from one delivers again what it had delivered
//! past its mark (the consumer drops those as duplicates) and never skips a row.
//!
//! The mark is checked before every pass. A row at its `seq` with another id is another session's
//! history (a restored backup), and the session is read again from its start. No row there is a
//! revert that took the marked row: a revert deletes rows from a boundary on and never rewinds the
//! sequence, so while `event_sequence` has not gone back below the mark every row before it is
//! the one delivered and reading goes on past it. A sequence that went back is a restored backup
//! too, read again from the start.
//!
//! # Sessions in both layouts
//!
//! On its first start opencode 2.0 copies every 1.x session into `session_v2`/`session_message`
//! (`packages/core/src/database/v1-migration.bun.ts`), deleting the whole `event` table first, and
//! keeps the 1.x tables, which 1.x goes on writing if it runs again. A copied row keeps the id of
//! the 1.x message it came from, except the synthetic text split off a user message, whose id keeps
//! the first 16 characters of it (`syntheticID`). The 1.x reader delivers those messages (as parts,
//! from the event log or the 1.x projection), so a row that is such a copy is skipped here: its id
//! is a 1.x message of the session, or it is synthetic and shares that prefix with one. What 2.0
//! wrote since is delivered here, what 1.x writes to a copied session afterwards by the 1.x reader,
//! and both layouts of a session are one [`Session`](crate::harnesstools::session::Session): see
//! [`checkpoint`] for how one checkpoint holds both places.
//!
//! [kenn-io/agentsview](https://github.com/kenn-io/agentsview)'s `opencode_v2.go` prefers
//! `session_v2` for a copied session instead; that would deliver again, under new ids, what 1.x
//! capture delivered as parts.
//!
//! # Forks
//!
//! A fork (`projectFork`) copies the parent's settled rows up to its boundary under ids
//! `<event>_<seq>` with the same `seq` and `data`, and the session names the parent in
//! `fork_session_id`. The copies are messages of the fork, and their model calls are the parent's:
//! the turn of a row with usage is keyed on what the copy keeps (see [`turn`]).

use std::collections::{BTreeSet, HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use futures::Stream;
use serde::de::Error as _;
use serde_json::Value;
use sqlx::sqlite::{SqliteConnectOptions, SqliteConnection, SqliteRow};
use sqlx::{Connection, Row, Sqlite};
use xxhash_rust::xxh3::xxh3_64;

use super::{
    Body, MessageInfo, Next, OpencodeMessage, Reads, epoch_millis, file_identity, json, text,
    tokens, usage_of,
};
use crate::db::sqlite::TransientResultCode;
use crate::db::sqlite::observe::{ObserveError, ReplayBehavior};
use crate::db::{query_as, query_scalar};
use crate::harnesstools::session::model::{
    Content, Role, StopReason, ToolCallId, ToolResult, ToolUse,
};
use crate::harnesstools::session::{Checkpoint, MessageError};
use crate::os::fs::FdIdentity;

/// The first line of the prompt a subagent session starts with (`tool/plugin/subagent.ts`,
/// `config/plugin/command.ts`): written by opencode, not typed by anyone.
const SUBAGENT_PROMPT: &str = "You are a subagent spawned by another session.";

/// Light rows a pass reads at a time.
const ROWS: usize = 256;

/// How often [`changes`] asks whether the database changed.
const POLL: Duration = Duration::from_millis(250);

/// A place in one layout of a session: the `seq` of the last row passed and a digest of its id.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Mark {
    pub(super) seq: i64,
    digest: Digest,
}

/// How much of an id's xxh3 a mark keeps: all of it, or the low half a checkpoint holding both
/// layouts has room for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Digest {
    Full(u64),
    Low(u32),
}

impl Mark {
    pub(super) fn of(seq: i64, id: &str) -> Self {
        Self {
            seq,
            digest: Digest::Full(xxh3_64(id.as_bytes())),
        }
    }

    /// Whether `id` is the id of the row this mark was taken at.
    pub(super) fn names(&self, id: &str) -> bool {
        let hash = xxh3_64(id.as_bytes());
        match self.digest {
            Digest::Full(digest) => hash == digest,
            Digest::Low(low) => Self::low_of(hash) == low,
        }
    }

    const fn low(&self) -> u32 {
        match self.digest {
            Digest::Full(hash) => Self::low_of(hash),
            Digest::Low(low) => low,
        }
    }

    #[allow(clippy::cast_possible_truncation, reason = "the low half is the point")]
    const fn low_of(hash: u64) -> u32 {
        hash as u32
    }
}

/// The bit a checkpoint holding both layouts sets.
const BOTH: u64 = 1 << 63;
/// How many bits of such a checkpoint hold the 2.0 layout's place.
const NEXT_BITS: u32 = 31;

/// The checkpoint of a session whose 1.x layout was read up to `legacy` and whose 2.0 layout up
/// to `next`.
///
/// A session read in its 1.x layout alone keeps the checkpoint it always had: the row's `seq`
/// and the full digest of its id, so a checkpoint stored before 2.0 support still resumes. Once
/// the 2.0 layout has a place, both are packed into one: the top bit set, then each place's `seq`
/// plus one (zero for none), the 1.x one above the 2.0 one, so that the checkpoint grows as
/// either place does; the digest holds the low halves of both ids' digests. A place too far on
/// to pack -- two billion events into one session -- is left out and read again.
pub(super) fn checkpoint(legacy: Option<(i64, &str)>, next: Option<Mark>) -> Checkpoint {
    let legacy = legacy.map(|(seq, id)| Mark::of(seq, id));
    let Some(next) = next else {
        return match legacy {
            Some(mark) => Checkpoint {
                at: u64::try_from(mark.seq).unwrap_or_default(),
                digest: match mark.digest {
                    Digest::Full(digest) => digest,
                    Digest::Low(low) => u64::from(low),
                },
            },
            None => Checkpoint {
                at: BOTH,
                digest: 0,
            },
        };
    };
    let plus_one = |mark: Option<Mark>, bits: u32| {
        mark.and_then(|mark| u64::try_from(mark.seq).ok())
            .map(|seq| seq + 1)
            .filter(|place| *place < 1 << bits)
            .zip(mark)
    };
    let (legacy_at, legacy_low) =
        plus_one(legacy, 32).map_or((0, 0), |(at, mark)| (at, mark.low()));
    let (next_at, next_low) =
        plus_one(Some(next), NEXT_BITS).map_or((0, 0), |(at, mark)| (at, mark.low()));
    Checkpoint {
        at: BOTH | legacy_at << NEXT_BITS | next_at,
        digest: u64::from(legacy_low) << 32 | u64::from(next_low),
    }
}

/// The places a checkpoint names in each layout (see [`checkpoint`]).
pub(super) fn unpack(from: Checkpoint) -> (Option<Mark>, Option<Mark>) {
    if from.at & BOTH == 0 {
        let legacy = i64::try_from(from.at).ok().map(|seq| Mark {
            seq,
            digest: Digest::Full(from.digest),
        });
        return (legacy, None);
    }
    let place = |at: u64, low: u64| {
        let seq = i64::try_from(at.checked_sub(1)?).ok()?;
        Some(Mark {
            seq,
            digest: Digest::Low(u32::try_from(low & 0xffff_ffff).unwrap_or_default()),
        })
    };
    let next_mask = (1 << NEXT_BITS) - 1;
    (
        place((from.at & !BOTH) >> NEXT_BITS, from.digest >> 32),
        place(from.at & next_mask, from.digest),
    )
}

/// One session's place in its 2.0 rows.
pub(super) struct Reader {
    session: String,
    reads: Arc<Reads>,
    /// The last row of the prefix of the session that has been delivered, or needed no delivery.
    mark: Option<Mark>,
    /// Rows past the mark that have been delivered already, by `seq`.
    delivered: BTreeSet<i64>,
    /// What the session info last delivered said, so that it is delivered again only once it
    /// changes.
    info: Option<SessionInfo>,
    /// Whether the last pass failed to read, so that it is tried again.
    failed: bool,
}

impl Reader {
    pub(super) fn from_start(session: String, reads: Arc<Reads>) -> Self {
        Self {
            session,
            reads,
            mark: None,
            delivered: BTreeSet::new(),
            info: None,
            failed: false,
        }
    }

    /// A reader past `mark`, as a tail started from now reads a session it saw before it started.
    pub(super) fn at(session: String, reads: Arc<Reads>, mark: Mark) -> Self {
        Self {
            mark: Some(mark),
            ..Self::from_start(session, reads)
        }
    }

    pub(super) const fn mark(&self) -> Option<Mark> {
        self.mark
    }

    pub(super) const fn failed(&self) -> bool {
        self.failed
    }

    /// A new consumer takes the session up: the session info is delivered to it again, first, so
    /// that it knows the title and parent whatever it has kept. `from` is where it resumes, if it
    /// names a place in this layout; the next pass checks that place (see the module docs).
    pub(super) fn reopen(&mut self, from: Option<Mark>) {
        self.info = None;
        if let Some(from) = from {
            self.mark = Some(from);
            self.delivered.clear();
        }
    }

    /// A pass over the rows this session has not delivered yet.
    pub(super) fn pass(&mut self) -> Pass<'_> {
        Pass {
            reader: self,
            stage: Stage::Start,
            rows: VecDeque::new(),
            cursor: -1,
            gap: false,
            exhausted: false,
            state: State::default(),
        }
    }

    /// Moves on past `row`: the mark, when every row before it is settled, else the rows
    /// delivered past the mark.
    fn passed(&mut self, seq: i64, id: &str, gap: bool) {
        if gap {
            self.delivered.insert(seq);
        } else {
            self.mark = Some(Mark::of(seq, id));
            self.delivered = self.delivered.split_off(&(seq + 1));
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    Start,
    Rows,
    Done,
}

/// One pass over a session's rows, from its mark to the last row there is.
pub(super) struct Pass<'a> {
    reader: &'a mut Reader,
    stage: Stage,
    /// What is left of the page of light rows being read.
    rows: VecDeque<Light>,
    /// The `seq` of the last row read, which the next page starts after.
    cursor: i64,
    /// Whether a row that is not settled yet has been passed over, which stops the mark.
    gap: bool,
    /// Whether the page being read is the last.
    exhausted: bool,
    state: State,
}

impl Pass<'_> {
    /// The session's next message and the mark it leaves the session at, or `None` when there is
    /// nothing more to read right now -- a failed read included (see [`Reader::failed`]).
    pub(super) async fn next(
        &mut self,
    ) -> Option<(Option<Mark>, Result<OpencodeMessage, MessageError>)> {
        loop {
            match self.stage {
                Stage::Done => return None,
                Stage::Start => {
                    self.stage = Stage::Done;
                    let reader = &mut *self.reader;
                    let Some(state) = reader.reads.state(&reader.session, reader.mark).await else {
                        reader.failed = true;
                        return None;
                    };
                    reader.failed = false;
                    let info = state.info.clone()?;
                    if state.restart {
                        tracing::warn!(
                            session = %reader.session,
                            "the row this session resumes at is no longer the one delivered; \
                             reading its 2.0 rows again from the start"
                        );
                        reader.mark = None;
                        reader.delivered.clear();
                    }
                    self.cursor = reader.mark.map_or(-1, |mark| mark.seq);
                    self.state = state;
                    self.stage = Stage::Rows;
                    if reader.info.as_ref().is_none_or(|said| !said.says(&info)) {
                        let message = info.message();
                        reader.info = Some(info);
                        return Some((reader.mark, Ok(message)));
                    }
                }
                Stage::Rows => {
                    let Some(row) = self.rows.pop_front() else {
                        if self.exhausted || !self.fill().await {
                            self.stage = Stage::Done;
                            return None;
                        }
                        continue;
                    };
                    self.cursor = row.seq;
                    if let Some(message) = self.row(row).await {
                        return Some(message);
                    }
                }
            }
        }
    }

    /// Reads the next page of light rows. `false` when that read failed.
    async fn fill(&mut self) -> bool {
        let reader = &mut *self.reader;
        let Some(rows) = reader.reads.rows(&reader.session, self.cursor).await else {
            reader.failed = true;
            return false;
        };
        self.exhausted = rows.len() < ROWS;
        self.rows = rows.into();
        true
    }

    /// The message `row` is, if it is due one now. `None` both for a row with no message and for a
    /// row that is not settled yet; a read that fails ends the pass.
    async fn row(
        &mut self,
        row: Light,
    ) -> Option<(Option<Mark>, Result<OpencodeMessage, MessageError>)> {
        let reader = &mut *self.reader;
        if reader.delivered.contains(&row.seq) {
            reader.passed(row.seq, &row.id, self.gap);
            return None;
        }
        if !row.settled {
            self.gap = true;
            return None;
        }
        if matches!(row.kind.as_str(), "agent-switched" | "model-switched" | "location-switched") {
            reader.passed(row.seq, &row.id, self.gap);
            return None;
        }
        let Some(full) = reader.reads.row(&reader.session, row.seq, self.state.legacy).await else {
            reader.failed = true;
            self.stage = Stage::Done;
            return None;
        };
        // deleted since the page was read (a revert), or a copy of a 1.x message
        let message = match full {
            Some(full) if full.id == row.id && !full.migrated => {
                let info = self.state.info.as_ref().expect("a pass reads rows of a session");
                full.message(info, &self.state)
            }
            _ => None,
        };
        reader.passed(row.seq, &row.id, self.gap);
        let message = message?;
        // the rows delivered before this one settled follow it into the mark it returns with
        while let Some(next) = self.rows.front().filter(|next| reader.delivered.contains(&next.seq))
        {
            reader.passed(next.seq, &next.id, self.gap);
            self.cursor = next.seq;
            self.rows.pop_front();
        }
        Some((reader.mark, message))
    }
}

/// What a pass needs to know of its session before it reads rows.
#[derive(Debug, Default)]
struct State {
    /// The session's row, `None` when the session has none (no 2.0 session, or deleted).
    info: Option<SessionInfo>,
    /// Whether the mark no longer holds (see the module docs).
    restart: bool,
    /// The session's moves, oldest first, for the directory each row was written in.
    moves: Vec<Moved>,
    /// Whether the database keeps opencode 1.x's `message` table, whose copies are skipped.
    legacy: bool,
}

impl State {
    /// The directory the session was in when the row at `seq` was written: where its last move
    /// before the row took it, else where its first move after the row took it from, else where
    /// it is.
    fn cwd_at(&self, seq: i64) -> Option<String> {
        self.moves
            .iter()
            .rev()
            .find(|moved| moved.seq < seq)
            .and_then(|moved| moved.to.clone())
            .or_else(|| {
                let first = self.moves.iter().find(|moved| moved.seq > seq)?;
                first.from.clone()
            })
            .or_else(|| self.info.as_ref()?.directory.clone())
    }
}

/// A `location-switched` row: where the session moved, and from where.
#[derive(Debug)]
struct Moved {
    seq: i64,
    to: Option<String>,
    from: Option<String>,
}

/// What a session's `session_v2` row says of it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct SessionInfo {
    id: String,
    title: Option<String>,
    parent: Option<String>,
    fork: Option<String>,
    directory: Option<String>,
    created: Option<i64>,
    updated: Option<i64>,
}

impl<'r> sqlx::FromRow<'r, SqliteRow> for SessionInfo {
    fn from_row(row: &'r SqliteRow) -> Result<Self, sqlx::Error> {
        Ok(Self {
            id: text(row, "id")?.unwrap_or_default(),
            title: text(row, "title")?,
            parent: text(row, "parent_id")?,
            fork: text(row, "fork_session_id")?,
            directory: text(row, "directory")?,
            created: row.try_get_unchecked("time_created")?,
            updated: row.try_get_unchecked("time_updated")?,
        })
    }
}

impl SessionInfo {
    /// Whether `other` says what this says of the session. Not when it was last updated: opencode
    /// bumps that on every prompt (`InboxEnqueued`), and an info repeating the title is a
    /// message the consumer drops.
    fn says(&self, other: &Self) -> bool {
        (&self.title, &self.parent, &self.fork, &self.directory)
            == (&other.title, &other.parent, &other.fork, &other.directory)
    }

    /// The session info as the message a 1.x `session.updated.1` row would be: its title, its
    /// directory, and the session it came from -- a subagent's parent, or the session a fork
    /// copied.
    fn message(&self) -> OpencodeMessage {
        let mut info = serde_json::json!({
            "id": self.id,
            "title": self.title,
            "directory": self.directory,
            "time": {"created": self.created, "updated": self.updated},
        });
        if let Some(parent) = self.parent.as_ref().or(self.fork.as_ref()) {
            info["parentID"] = Value::from(parent.as_str());
        }
        OpencodeMessage::session(serde_json::json!({ "info": info }))
            .expect("the info is an object")
    }
}

/// What a pass reads of a row to know whether it is due a message.
#[derive(Debug)]
struct Light {
    seq: i64,
    id: String,
    kind: String,
    settled: bool,
}

impl<'r> sqlx::FromRow<'r, SqliteRow> for Light {
    fn from_row(row: &'r SqliteRow) -> Result<Self, sqlx::Error> {
        let settled: Option<i64> = row.try_get_unchecked("settled")?;
        Ok(Self {
            seq: row.try_get_unchecked("seq")?,
            id: text(row, "id")?.unwrap_or_default(),
            kind: text(row, "type")?.unwrap_or_default(),
            settled: settled.is_none_or(|settled| settled != 0),
        })
    }
}

/// A row whole.
#[derive(Debug)]
struct Full {
    seq: i64,
    id: String,
    kind: String,
    created: Option<i64>,
    data: Option<String>,
    /// Whether the row is opencode's copy of a 1.x message (see the module docs).
    migrated: bool,
}

impl<'r> sqlx::FromRow<'r, SqliteRow> for Full {
    fn from_row(row: &'r SqliteRow) -> Result<Self, sqlx::Error> {
        let migrated: Option<i64> = row.try_get_unchecked("migrated")?;
        Ok(Self {
            seq: row.try_get_unchecked("seq")?,
            id: text(row, "id")?.unwrap_or_default(),
            kind: text(row, "type")?.unwrap_or_default(),
            created: row.try_get_unchecked("time_created")?,
            data: text(row, "data")?,
            migrated: migrated.is_some_and(|migrated| migrated != 0),
        })
    }
}

/// The model call a row with usage is: when it started, with which model, and what it counted.
///
/// Not the row's id, which a fork's copy does not keep: the copy keeps the row's `data` whole,
/// and with it all of this. It is also the key opencode 1.x's reader gives a one-step assistant
/// message's `step-finish` (`MessageInfo::turn_of` and the counts `OpencodeMessage::turn_id`
/// appends), which the V1 migration copies into one row with the same `time.created`, model and
/// `tokens`: a fork 2.0 makes of a copied session counts none of those calls again.
fn turn(created: Option<i64>, model: &Value, of: &str, counts: &Value) -> String {
    let created = created.unwrap_or_default();
    let mut turn = match (model["providerID"].as_str(), model["id"].as_str()) {
        (Some(provider), Some(model)) => format!("{created}:{provider}/{model}"),
        _ => format!("{created}:{of}"),
    };
    if counts.is_object() {
        let count = |value: &Value| tokens(value).unwrap_or_default();
        turn.push_str(&format!(
            "#{}/{}/{}/{}/{}",
            count(&counts["input"]),
            count(&counts["output"]),
            count(&counts["reasoning"]),
            count(&counts["cache"]["read"]),
            count(&counts["cache"]["write"]),
        ));
    }
    turn
}

/// What an opencode 2.0 error (`SessionError.Error`: `{type, message}`) says happened.
fn error_text(error: &Value) -> String {
    match error["message"].as_str() {
        Some(message) if !message.is_empty() => message.to_owned(),
        _ => error["type"].as_str().unwrap_or("error").to_owned(),
    }
}

/// Why a model call that reports `error` ended: `aborted` is what an interrupted step, tool or
/// declined permission reports (`runner/step.ts`), and a `content-filter` finish is surfaced as
/// `provider.content-filter` (`publish-llm-event.ts`).
fn stop_of_error(error: &Value) -> StopReason {
    match error["type"].as_str() {
        Some("aborted") => StopReason::Aborted,
        Some("provider.content-filter") => StopReason::Refusal,
        _ => StopReason::Error,
    }
}

impl Full {
    /// The message this row is, `None` for one with none: a session going idle after a turn that
    /// succeeded, and the switches of agent, model and location.
    fn message(
        self,
        info: &SessionInfo,
        state: &State,
    ) -> Option<Result<OpencodeMessage, MessageError>> {
        let data = match self.data.as_deref().map(json) {
            Some(Ok(data)) if data.is_object() => data,
            Some(Err(err)) => return Some(Err(MessageError::from(err))),
            _ => {
                let err = serde_json::Error::custom("a session_message row's data is no object");
                return Some(Err(MessageError::from(err)));
            }
        };
        let time = epoch_millis(&data["time"]["created"]).or(self.created);
        let said = |role: Role, content: Vec<Content>| Next {
            role,
            id: self.id.clone(),
            content,
            usage: None,
            stop: None,
        };
        let text = |key: &str| data[key].as_str().unwrap_or_default().to_owned();
        let (next, info) = match self.kind.as_str() {
            "user" => {
                let prompt = text("text");
                let role = if prompt.starts_with(SUBAGENT_PROMPT) {
                    Role::System
                } else {
                    Role::User
                };
                (said(role, vec![Content::Text(prompt)]), None)
            }
            // text opencode wrote into the conversation itself
            "synthetic" | "system" | "skill" => {
                (said(Role::System, vec![Content::Text(text("text"))]), None)
            }
            // a command the user ran with `!`, whose output is no conversation
            "shell" => {
                let command = format!("! {}", text("command"));
                (said(Role::User, vec![Content::Text(command)]), None)
            }
            "assistant" => {
                let (next, learned) = self.assistant(&data, state);
                (next, Some(learned))
            }
            "compaction" => {
                let (next, learned) = self.compaction(&data, state);
                (next, Some(learned))
            }
            "idle" => {
                let stop = match data["outcome"].as_str() {
                    Some("failed") => StopReason::Error,
                    Some("interrupted") => StopReason::Aborted,
                    _ => return None,
                };
                let next = Next {
                    stop: Some(stop),
                    ..said(Role::System, Vec::new())
                };
                (next, None)
            }
            "agent-switched" | "model-switched" | "location-switched" => return None,
            other => {
                tracing::debug!(session = %info.id, kind = other, "delivering a session_message row whole");
                let mut data = data;
                data["id"] = Value::from(self.id.as_str());
                return Some(Ok(OpencodeMessage {
                    role: Role::Other(other.to_owned()),
                    body: Body::Raw(data),
                    time,
                    info: None,
                }));
            }
        };
        Some(Ok(OpencodeMessage {
            role: next.role.clone(),
            body: Body::Next(next),
            time,
            info,
        }))
    }

    /// What the call's message needs to say of it: an assistant's, whose model, cwd and turn a
    /// row of an assistant carries.
    fn learned(&self, data: &Value, model: &Value, of: &str, state: &State) -> MessageInfo {
        MessageInfo {
            role: Role::Assistant,
            turn: Some(turn(epoch_millis(&data["time"]["created"]), model, of, &data["tokens"])),
            model: model["id"].as_str().map(str::to_owned),
            cwd: state.cwd_at(self.seq),
            ..MessageInfo::unknown()
        }
    }

    /// One model call: what it said, reasoned and called, how it ended and what it cost.
    fn assistant(&self, data: &Value, state: &State) -> (Next, MessageInfo) {
        let mut content = Vec::new();
        for item in data["content"].as_array().into_iter().flatten() {
            match item["type"].as_str() {
                Some("text") => {
                    content
                        .push(Content::Text(item["text"].as_str().unwrap_or_default().to_owned()));
                }
                Some("reasoning") => content.push(Content::ReasoningSummary { tokens: None }),
                Some("tool") => {
                    let call = ToolCallId::from(item["id"].as_str().unwrap_or_default().to_owned());
                    let state = &item["state"];
                    content.push(Content::ToolUse(ToolUse {
                        id: call.clone(),
                        name: item["name"].as_str().unwrap_or_default().to_owned(),
                        input: state["input"].clone(),
                    }));
                    match state["status"].as_str() {
                        Some("completed") => content.push(Content::ToolResult(ToolResult {
                            call,
                            output: state["content"].clone(),
                            error: false,
                        })),
                        Some("error") => content.push(Content::ToolResult(ToolResult {
                            call,
                            output: Value::from(error_text(&state["error"])),
                            error: true,
                        })),
                        // a call the step never settled
                        _ => {}
                    }
                }
                _ => content.push(Content::Other(item.clone())),
            }
        }
        let error = &data["error"];
        if error.is_object() {
            content.push(Content::Error(error_text(error)));
        }
        let stop = if error.is_object() {
            Some(stop_of_error(error))
        } else {
            data["finish"].as_str().map(OpencodeMessage::stop_reason_of)
        };
        let next = Next {
            role: Role::Assistant,
            id: self.id.clone(),
            content,
            usage: usage_of(&data["tokens"]),
            stop,
        };
        (next, self.learned(data, &data["model"], "assistant", state))
    }

    /// A compaction: the summary standing in for the conversation before it, or why none was
    /// written, and the usage of the calls that wrote it.
    fn compaction(&self, data: &Value, state: &State) -> (Next, MessageInfo) {
        let (content, stop) = if data["status"] == "failed" {
            (vec![Content::Error(error_text(&data["error"]))], Some(StopReason::Error))
        } else {
            (vec![Content::Summary(data["summary"].as_str().unwrap_or_default().to_owned())], None)
        };
        let next = Next {
            role: Role::Assistant,
            id: self.id.clone(),
            content,
            usage: usage_of(&data["tokens"]),
            stop,
        };
        (next, self.learned(data, &data["model"], "compaction", state))
    }
}

/// The tables a read of the 2.0 layout looks for.
async fn tables(conn: &mut SqliteConnection) -> Result<Vec<String>, sqlx::Error> {
    query_scalar::<Sqlite, String>(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name IN ('session_v2', \
         'session_message', 'event_sequence', 'message')",
    )
    .fetch_all(conn)
    .await
}

impl Reads {
    /// What a pass needs to know of `session` before reading its rows (see [`State`]), checking
    /// `mark` on the way.
    async fn state(self: &Arc<Self>, session: &str, mark: Option<Mark>) -> Option<State> {
        let session = session.to_owned();
        self.read("2.0 session read", move |conn| {
            Box::pin(async move {
                let tables = tables(&mut *conn).await?;
                let has = |name: &str| tables.iter().any(|table| table == name);
                if !has("session_v2") || !has("session_message") {
                    return Ok(State::default());
                }
                let info = query_as::<Sqlite, SessionInfo>(
                    "SELECT id, title, parent_id, fork_session_id, directory, time_created, \
                     time_updated FROM session_v2 WHERE id = ?1",
                )
                .bind(&session)
                .fetch_optional(&mut *conn)
                .await?;
                if info.is_none() {
                    return Ok(State::default());
                }
                let restart = match mark {
                    None => false,
                    Some(mark) => {
                        let at = query_scalar::<Sqlite, Option<String>>(
                            "SELECT CAST(id AS TEXT) FROM session_message WHERE session_id = ?1 \
                             AND seq = ?2",
                        )
                        .bind(&session)
                        .bind(mark.seq)
                        .fetch_optional(&mut *conn)
                        .await?;
                        match at {
                            Some(id) => !mark.names(id.as_deref().unwrap_or_default()),
                            // reverted, unless the sequence itself went back
                            None if has("event_sequence") => {
                                let head = query_scalar::<Sqlite, i64>(
                                    "SELECT seq FROM event_sequence WHERE aggregate_id = ?1",
                                )
                                .bind(&session)
                                .fetch_optional(&mut *conn)
                                .await?;
                                head.is_none_or(|head| head < mark.seq)
                            }
                            None => true,
                        }
                    }
                };
                let moves = query_as::<Sqlite, (i64, Option<String>)>(
                    "SELECT seq, CAST(data AS TEXT) FROM session_message WHERE session_id = ?1 \
                     AND type = 'location-switched' ORDER BY seq",
                )
                .bind(&session)
                .fetch_all(&mut *conn)
                .await?
                .into_iter()
                .map(|(seq, data)| {
                    let data = data.as_deref().and_then(|data| json(data).ok()).unwrap_or_default();
                    let dir = |value: &Value| value.as_str().map(str::to_owned);
                    Moved {
                        seq,
                        to: dir(&data["location"]["directory"]),
                        from: dir(&data["previous"]["location"]["directory"]),
                    }
                })
                .collect();
                Ok(State {
                    info,
                    restart,
                    moves,
                    legacy: has("message"),
                })
            })
        })
        .await
    }

    /// The light rows of `session` past `after`, in `seq` order: whether each is settled is
    /// worked out in SQL, so that the payloads of rows delivered already or still being written
    /// stay in the table. A payload that is not JSON counts as settled, so that its error is
    /// delivered rather than waited on.
    async fn rows(self: &Arc<Self>, session: &str, after: i64) -> Option<Vec<Light>> {
        let session = session.to_owned();
        self.read("2.0 row page read", move |conn| {
            Box::pin(async move {
                query_as::<Sqlite, Light>(
                    "SELECT id, seq, type, CASE WHEN json_valid(data) = 0 THEN 1 WHEN type = \
                     'assistant' THEN json_extract(data, '$.time.completed') IS NOT NULL WHEN \
                     type = 'compaction' THEN coalesce(json_extract(data, '$.status'), '') != \
                     'running' ELSE 1 END AS settled FROM session_message WHERE session_id = ?1 \
                     AND seq > ?2 ORDER BY seq LIMIT ?3",
                )
                .bind(session)
                .bind(after)
                .bind(i64::try_from(ROWS).expect("ROWS fits an i64"))
                .fetch_all(conn)
                .await
            })
        })
        .await
    }

    /// The row of `session` at `seq`, whole, and whether it is a copy of a 1.x message when the
    /// database keeps 1.x's `message` table (`legacy`). `Some(None)` for a row gone since.
    async fn row(self: &Arc<Self>, session: &str, seq: i64, legacy: bool) -> Option<Option<Full>> {
        const PLAIN: &str = "SELECT id, seq, type, time_created, data, 0 AS migrated FROM \
                             session_message WHERE session_id = ?1 AND seq = ?2";
        // `syntheticID` keeps the first 16 characters of the 1.x message it splits off
        const LEGACY: &str = "SELECT m.id, m.seq, m.type, m.time_created, m.data, EXISTS (SELECT \
                              1 FROM message l WHERE l.session_id = m.session_id AND (l.id = m.id \
                              OR (m.type = 'synthetic' AND substr(l.id, 1, 16) = substr(m.id, 1, \
                              16)))) AS migrated FROM session_message m WHERE m.session_id = ?1 \
                              AND m.seq = ?2";
        let session = session.to_owned();
        self.read("2.0 row read", move |conn| {
            Box::pin(async move {
                query_as::<Sqlite, Full>(if legacy {
                    LEGACY
                } else {
                    PLAIN
                })
                .bind(session)
                .bind(seq)
                .fetch_optional(conn)
                .await
            })
        })
        .await
    }

    /// Every 2.0 session, oldest first; none without a `session_v2` table.
    pub(super) async fn v2_sessions(self: &Arc<Self>) -> Option<Vec<String>> {
        self.read("2.0 session scan", |conn| {
            Box::pin(async move {
                if !tables(&mut *conn).await?.iter().any(|table| table == "session_v2") {
                    return Ok(Vec::new());
                }
                let ids = query_scalar::<Sqlite, Option<String>>(
                    "SELECT CAST(id AS TEXT) FROM session_v2 ORDER BY time_created, id",
                )
                .fetch_all(conn)
                .await?;
                Ok(ids.into_iter().flatten().collect())
            })
        })
        .await
    }
}

/// A 2.0 session that changed: it wrote a row, or rewrote one, or was renamed, moved or reverted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Change {
    pub(super) session: String,
    /// Where a tail started from now takes up a session it saw before it started.
    pub(super) from: Option<Mark>,
}

/// The 2.0 sessions of `db` as they change (see the module docs), each change once per session
/// however many writes it took, and every session on the first scan when `replay` is
/// [`ReplayBehavior::All`].
///
/// A failure is an item, reported once however many polls in a row fail, and the poll carries on
/// after it on a new connection.
pub(super) fn changes(
    db: PathBuf,
    replay: ReplayBehavior,
) -> impl Stream<Item = Result<Change, ObserveError>> + Send + 'static {
    async_stream::stream! {
        let mut poll = Poll::new(db, replay);
        let mut failing = false;
        let mut ticker = tokio::time::interval(POLL);
        loop {
            ticker.tick().await;
            match poll.once().await {
                Ok(changes) => {
                    failing = false;
                    for change in changes {
                        yield Ok(change);
                    }
                }
                Err(err) if failing => tracing::debug!(%err, "opencode 2.0's sessions still cannot be polled"),
                Err(err) => {
                    failing = true;
                    yield Err(err);
                }
            }
        }
    }
}

/// What [`changes`] keeps between polls.
struct Poll {
    db: PathBuf,
    replay: ReplayBehavior,
    /// A connection of its own, since `PRAGMA data_version` only moves for other connections'
    /// commits, and the identity of the file it was opened on.
    open: Option<(SqliteConnection, Option<FdIdentity>)>,
    /// `PRAGMA data_version` as the last scan found it.
    version: Option<i64>,
    /// Each session's event sequence as last seen.
    seen: HashMap<String, i64>,
    /// Where a tail started from now takes up each session it saw before it started.
    seeds: HashMap<String, Mark>,
    first: bool,
}

impl Poll {
    fn new(db: PathBuf, replay: ReplayBehavior) -> Self {
        Self {
            db,
            replay,
            open: None,
            version: None,
            seen: HashMap::new(),
            seeds: HashMap::new(),
            first: true,
        }
    }

    /// The sessions that changed since the last poll; none while the database is locked.
    async fn once(&mut self) -> Result<Vec<Change>, ObserveError> {
        let identity = file_identity(&self.db);
        if self.open.as_ref().is_some_and(|(_, opened)| *opened != identity) {
            tracing::warn!(db = %self.db.display(), "opencode's database file was replaced; reconnecting");
            self.open = None;
            self.version = None;
        }
        if self.open.is_none() {
            let opts = SqliteConnectOptions::new()
                .filename(&self.db)
                .read_only(true)
                .busy_timeout(Duration::from_secs(5));
            let conn =
                SqliteConnection::connect_with(&opts).await.map_err(ObserveError::Connect)?;
            self.open = Some((conn, identity));
        }
        let conn = &mut self.open.as_mut().expect("opened above").0;
        let from_now = self.first && self.replay == ReplayBehavior::FromNow;
        let scanned = async {
            let now =
                query_scalar::<Sqlite, i64>("PRAGMA data_version").fetch_one(&mut *conn).await?;
            if self.version == Some(now) {
                return Ok(None);
            }
            Ok(Some((now, scan(conn, from_now).await?)))
        };
        let (now, (sessions, seeds)) = match scanned.await {
            Ok(Some(scanned)) => scanned,
            Ok(None) => return Ok(Vec::new()),
            Err(err) if TransientResultCode::from_error(&err).is_some() => return Ok(Vec::new()),
            Err(err) => {
                self.open = None;
                return Err(ObserveError::Query(err));
            }
        };
        self.version = Some(now);
        self.first = false;
        if from_now {
            self.seen = sessions.into_iter().collect();
            self.seeds = seeds;
            return Ok(Vec::new());
        }
        let mut changes = Vec::new();
        for (session, seq) in sessions {
            if self.seen.insert(session.clone(), seq) != Some(seq) {
                let from = self.seeds.remove(&session);
                changes.push(Change { session, from });
            }
        }
        Ok(changes)
    }
}

/// Each 2.0 session with its event sequence and, when `seed`, the last row of each.
async fn scan(
    conn: &mut SqliteConnection,
    seed: bool,
) -> Result<(Vec<(String, i64)>, HashMap<String, Mark>), sqlx::Error> {
    let tables = tables(&mut *conn).await?;
    let has = |name: &str| tables.iter().any(|table| table == name);
    if !has("session_v2") || !has("session_message") || !has("event_sequence") {
        return Ok((Vec::new(), HashMap::new()));
    }
    let sessions: Vec<(Option<String>, i64)> = query_as(
        "SELECT CAST(s.id AS TEXT), coalesce(e.seq, -1) FROM session_v2 s LEFT JOIN \
         event_sequence e ON e.aggregate_id = s.id",
    )
    .fetch_all(&mut *conn)
    .await?;
    let sessions = sessions.into_iter().filter_map(|(id, seq)| Some((id?, seq))).collect();
    if !seed {
        return Ok((sessions, HashMap::new()));
    }
    let last: Vec<(Option<String>, i64, Option<String>)> = query_as(
        "SELECT CAST(m.session_id AS TEXT), m.seq, CAST(m.id AS TEXT) FROM session_message m JOIN \
         (SELECT session_id, max(seq) AS seq FROM session_message GROUP BY session_id) t ON \
         t.session_id = m.session_id AND t.seq = m.seq",
    )
    .fetch_all(conn)
    .await?;
    let seeds = last
        .into_iter()
        .filter_map(|(session, seq, id)| Some((session?, Mark::of(seq, &id.unwrap_or_default()))))
        .collect();
    Ok((sessions, seeds))
}

#[cfg(test)]
mod tests;
