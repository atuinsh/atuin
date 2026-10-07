//! Writing a harness-native transcript back out from captured messages, so a session recorded on
//! another machine (or whose transcript was deleted) can be resumed here.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use time::OffsetDateTime;

use crate::harnesstools::note::{Part, render, tool_note};
use crate::harnesstools::session::{Content, Role, StopReason, ToolCallId, Usage};

/// One captured message, harness-agnostic, in transcript order.
#[derive(Clone, Debug)]
pub struct RehydrateMessage {
    /// The harness's own id for the line/part this row came from (capture's `source_id`). Written
    /// back verbatim so re-capturing the rehydrated transcript dedups against the synced rows.
    pub source_id: String,
    pub parent_source_id: Option<String>,
    pub timestamp: OffsetDateTime,
    pub role: Role,
    pub content: Vec<Content>,
    pub model: Option<String>,
    pub usage: Option<Usage>,
    pub stop_reason: Option<StopReason>,
    pub turn_id: Option<String>,
    pub cwd: Option<PathBuf>,
    pub git_branch: Option<String>,
}

/// A session to write back out.
#[derive(Clone, Debug)]
pub struct RehydrateSession {
    pub id: String,
    pub title: Option<String>,
    /// Where the session will be resumed on this machine (may differ from the original cwd).
    pub cwd: PathBuf,
    pub original_cwd: Option<PathBuf>,
    pub git_branch: Option<String>,
    pub model: Option<String>,
    pub started_at: OffsetDateTime,
    pub messages: Vec<RehydrateMessage>,
    /// Set on a fork ([`crate::harnesstools::fork`]): the session it was forked from, which the
    /// writer links it to the way its harness links a fork of its own.
    pub fork_of: Option<ForkOf>,
}

/// The session a fork was forked from, as its writer names it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForkOf {
    /// Its native id.
    pub id: String,
    /// Its atuin id, where the link is text that can say it (opencode's marker).
    pub atuin_id: Option<String>,
    /// Its transcript on this machine: pi names a fork's parent by its file, and a Codex fork
    /// continues the history the original's rollout continues (its `history_base`).
    pub path: Option<PathBuf>,
}

#[derive(Debug, thiserror::Error)]
pub enum RehydrateError {
    #[error("{0} sessions can't be rehydrated")]
    Unsupported(&'static str),
    #[error("a transcript for this session already exists at {0}")]
    AlreadyExists(PathBuf),
    #[error("the agent's data directory could not be found")]
    NoDataDir,
    #[error("rehydrating failed: {0}")]
    Other(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// What a restored tool result says when capture kept no output (`ai.capture_tools` off, a policy
/// withheld it, or it was over the size limit): the formats all want some output for a result, and
/// an empty one would tell the model the tool printed nothing.
pub const UNCAPTURED_OUTPUT: &str = "(output not captured)";

/// How a writer lays out the calls [`flatten_uncaptured_calls`] turns into notes.
pub enum Flatten<'f> {
    /// Every run of assistant rows holding such a call (the rows between two the writer sends as
    /// something else: a prompt, a result it keeps) is merged into the run's first row, its text
    /// and notes rendered as one text, so the run stays one assistant message once the results
    /// between its rows are dropped. For formats that write every row under its source id, and
    /// whose harnesses send consecutive assistant rows as separate messages (Claude Code, pi).
    Runs,
    /// Each call's note joins the text of the latest assistant row before it that `host` takes
    /// (given the host and the call's row), with nothing but reasoning, empty rows or other
    /// notes between; else it is written as text in place of the call, in the call's own row,
    /// when `own` allows that row to change; else it is left out. For formats whose harnesses
    /// send consecutive assistant messages as they are (Codex, opencode), or where a row cannot
    /// change its content and keep its id.
    Notes {
        host: &'f dyn Fn(&RehydrateMessage, &RehydrateMessage) -> bool,
        own: &'f dyn Fn(&RehydrateMessage) -> bool,
    },
}

/// A tool call capture kept without its input (`ai.capture_tools` off, a policy withheld it, or it
/// was over the size limit): only its name is synced.
#[must_use]
pub fn is_uncaptured(content: &Content) -> bool {
    matches!(content, Content::ToolUse(call) if call.input.is_null())
}

/// `messages` with every tool call captured without its input turned into a note in the text of
/// its assistant turn ([`tool_note`]: `[ran a shell command]`, the same one several times in a
/// row counted, `×3`), and its result (and any patch of it) dropped.
///
/// No harness's API takes a call without its input (Claude's `tool_use.input` must be an object,
/// a Codex `function_call`'s `arguments` a JSON string, ...), and one made up would tell the model
/// it called a tool on nothing. Calls kept with their input stay calls.
///
/// Rows keep their source ids: a row whose content changed reads back under its own id, which
/// capture already holds, so re-capturing the transcript pushes nothing for it; a row merged away
/// is no longer written, so re-capture never sees it. Rows left empty stay in place (writers skip
/// them), so the tree still links through them.
#[must_use]
pub fn flatten_uncaptured_calls(
    messages: &[RehydrateMessage],
    how: &Flatten<'_>,
) -> Vec<RehydrateMessage> {
    let calls: HashSet<ToolCallId> = messages
        .iter()
        .filter(|m| m.role == Role::Assistant)
        .flat_map(|m| &m.content)
        .filter_map(|c| match c {
            Content::ToolUse(call) if call.input.is_null() => Some(call.id.clone()),
            _ => None,
        })
        .collect();
    let mut rows = messages.to_vec();
    if calls.is_empty() {
        return rows;
    }
    for row in &mut rows {
        row.content.retain(|c| match c {
            Content::ToolResult(r) => !calls.contains(&r.call),
            Content::Patch(p) => !calls.contains(&p.call),
            _ => true,
        });
    }
    match how {
        Flatten::Runs => merge_runs(&mut rows),
        Flatten::Notes { host, own } => join_notes(&mut rows, host, own),
    }
    rows
}

/// A piece of a flattened row: text or a note (rendered together), or content kept as it is.
enum Piece {
    Part(Part),
    Kept(Content),
}

fn pieces(content: Vec<Content>) -> impl Iterator<Item = Piece> {
    content.into_iter().filter_map(|c| match c {
        Content::Text(text) if text.trim().is_empty() => None,
        Content::Text(text) => Some(Piece::Part(Part::Text(text))),
        Content::ToolUse(call) if call.input.is_null() => {
            Some(Piece::Part(Part::Note(tool_note(&call.name, &call.input))))
        }
        other => Some(Piece::Kept(other)),
    })
}

/// `pieces` as content: each stretch of text and notes one text block.
fn assemble(pieces: impl IntoIterator<Item = Piece>) -> Vec<Content> {
    let mut content = Vec::new();
    let mut parts = Vec::new();
    for piece in pieces {
        match piece {
            Piece::Part(part) => parts.push(part),
            Piece::Kept(kept) => {
                if !parts.is_empty() {
                    content.push(Content::Text(render(&std::mem::take(&mut parts))));
                }
                content.push(kept);
            }
        }
    }
    if !parts.is_empty() {
        content.push(Content::Text(render(&parts)));
    }
    content
}

fn is_reasoning(c: &Content) -> bool {
    matches!(c, Content::Reasoning(_) | Content::ReasoningSummary { .. })
}

fn failed(m: &RehydrateMessage) -> bool {
    m.content.iter().any(|c| matches!(c, Content::Error(_)))
}

/// [`Flatten::Runs`].
fn merge_runs(rows: &mut [RehydrateMessage]) {
    let mut at = 0;
    while at < rows.len() {
        if rows[at].role != Role::Assistant {
            at += 1;
            continue;
        }
        let end = rows[at..]
            .iter()
            .position(|m| m.role != Role::Assistant && !m.content.is_empty())
            .map_or(rows.len(), |n| at + n);
        let run = &mut rows[at..end];
        if run.iter().any(|m| m.content.iter().any(is_uncaptured)) {
            merge_run(run);
        }
        at = end;
    }
}

fn merge_run(run: &mut [RehydrateMessage]) {
    // A failed call's row stays where it is (the harnesses leave those out of what they send the
    // model), its calls flattened in place: merged, its error would fail the whole turn.
    for row in run.iter_mut().filter(|m| m.role == Role::Assistant && failed(m)) {
        row.content = assemble(pieces(std::mem::take(&mut row.content)));
    }
    let merged: Vec<usize> =
        (0..run.len()).filter(|&i| run[i].role == Role::Assistant && !failed(&run[i])).collect();
    let (Some(&host), Some(&last)) = (merged.first(), merged.last()) else {
        return;
    };
    let last_stop = run[last].stop_reason.clone();
    let mut all = Vec::new();
    for &i in &merged {
        // Neither writer keeps reasoning (capture keeps only that there was some): left in, it
        // would split the text around it.
        let content = std::mem::take(&mut run[i].content);
        all.extend(pieces(content.into_iter().filter(|c| !is_reasoning(c)).collect()));
    }
    let host = &mut run[host];
    host.content = assemble(all);
    // The turn ends as its last row did, but not on a call it no longer makes.
    let calls = host.content.iter().any(|c| matches!(c, Content::ToolUse(_)));
    host.stop_reason = match last_stop {
        Some(StopReason::ToolUse) if !calls => Some(StopReason::EndTurn),
        other => other,
    };
}

/// [`Flatten::Notes`].
fn join_notes(
    rows: &mut [RehydrateMessage],
    host_ok: &dyn Fn(&RehydrateMessage, &RehydrateMessage) -> bool,
    own_ok: &dyn Fn(&RehydrateMessage) -> bool,
) {
    let text_parts = |m: &RehydrateMessage| -> Option<Vec<Part>> {
        let text = !m.content.is_empty() && m.content.iter().all(|c| matches!(c, Content::Text(_)));
        text.then(|| {
            m.content
                .iter()
                .filter_map(|c| match c {
                    Content::Text(t) if !t.trim().is_empty() => Some(Part::Text(t.clone())),
                    _ => None,
                })
                .collect()
        })
    };
    // The row notes join, and its text so far.
    let mut host: Option<(usize, Vec<Part>)> = None;
    for i in 0..rows.len() {
        if rows[i].role != Role::Assistant {
            if !rows[i].content.is_empty() {
                host = None;
            }
            continue;
        }
        if !rows[i].content.iter().any(is_uncaptured) {
            if let Some(parts) = text_parts(&rows[i]) {
                host = Some((i, parts));
            } else if !rows[i].content.iter().all(is_reasoning) {
                host = None;
            }
            continue;
        }
        let only_calls = rows[i].content.iter().all(|c| is_uncaptured(c) || is_reasoning(c));
        if only_calls
            && let Some((h, parts)) = &mut host
            && host_ok(&rows[*h], &rows[i])
        {
            parts.extend(pieces(std::mem::take(&mut rows[i].content)).filter_map(|p| match p {
                Piece::Part(part) => Some(part),
                Piece::Kept(_) => None,
            }));
            rows[*h].content = vec![Content::Text(render(parts))];
        } else if own_ok(&rows[i]) {
            let content = std::mem::take(&mut rows[i].content);
            let all: Vec<Piece> =
                pieces(content.into_iter().filter(|c| !is_reasoning(c)).collect()).collect();
            if all.iter().all(|p| matches!(p, Piece::Part(_))) {
                let parts: Vec<Part> = all
                    .into_iter()
                    .filter_map(|p| match p {
                        Piece::Part(part) => Some(part),
                        Piece::Kept(_) => None,
                    })
                    .collect();
                rows[i].content = vec![Content::Text(render(&parts))];
                host = Some((i, parts));
            } else {
                rows[i].content = assemble(all);
                host = None;
            }
        } else {
            rows[i].content.retain(|c| !is_uncaptured(c));
            if !rows[i].content.iter().all(is_reasoning) {
                host = None;
            }
        }
    }
}

/// The rows on a parent cycle (a corrupt transcript's), of rows by source id. A writer links
/// such a row only to a line already written: linking along the cycle would close it.
pub(crate) fn on_parent_cycles<'a>(
    by_id: &HashMap<&'a str, &'a RehydrateMessage>,
) -> HashSet<&'a str> {
    let mut on_cycle = HashSet::new();
    let mut done = HashSet::new();

    for &start in by_id.keys() {
        let mut walk = Vec::new();
        let mut on_walk = HashMap::new();
        let mut at = Some(start);
        while let Some(id) = at.filter(|id| !done.contains(id)) {
            if let Some(&from) = on_walk.get(id) {
                on_cycle.extend(walk[from..].iter().copied());
                break;
            }

            on_walk.insert(id, walk.len());
            walk.push(id);
            at = by_id.get(id).and_then(|r| r.parent_source_id.as_deref());
        }
        done.extend(walk);
    }

    on_cycle
}

/// Break any parent cycle left among `lines`, the JSON lines a writer wrote, each naming its own
/// id under `id_key` and its parent under the first of `link_keys` holding one. Parents are
/// resolved however the rows are ordered, so a corrupt cycle and clock skew can still close one;
/// it is cut at its earliest line, re-pointed to the nearest line before it that doesn't lead
/// back in, else made a root.
pub(crate) fn break_line_cycles(lines: &mut [serde_json::Value], id_key: &str, link_keys: &[&str]) {
    let link = |l: &serde_json::Value| link_keys.iter().copied().find(|k| l[*k].is_string());
    let index: HashMap<&str, usize> =
        lines.iter().enumerate().filter_map(|(i, l)| Some((l[id_key].as_str()?, i))).collect();
    let mut parent: Vec<Option<usize>> =
        lines.iter().map(|l| link(l).and_then(|k| index.get(l[k].as_str()?).copied())).collect();
    let n = parent.len();

    // 0: unseen, 1: on the walk in progress, 2: known to reach a root.
    let mut state = vec![0_u8; n];
    let mut walk = Vec::new();
    let mut cuts = Vec::new();
    for start in 0..n {
        walk.clear();
        let mut at = Some(start);
        while let Some(i) = at {
            match state[i] {
                2 => break,
                1 => {
                    let from = walk.iter().position(|&w| w == i).expect("on the walk");
                    let cycle = &walk[from..];
                    let earliest = *cycle.iter().min().expect("non-empty");
                    let leads_in = |mut at: Option<usize>| {
                        for _ in 0..n {
                            let Some(j) = at else {
                                return false;
                            };
                            if cycle.contains(&j) {
                                return true;
                            }
                            at = parent[j];
                        }
                        true
                    };
                    let to = (0..earliest).rev().find(|&j| !leads_in(Some(j)));

                    parent[earliest] = to;
                    cuts.push(earliest);
                    break;
                }
                _ => {
                    state[i] = 1;
                    walk.push(i);
                    at = parent[i];
                }
            }
        }
        for &i in &walk {
            state[i] = 2;
        }
    }

    for cut in cuts {
        let to = parent[cut].map_or(serde_json::Value::Null, |j| lines[j][id_key].clone());
        if let Some(key) = link(&lines[cut]) {
            lines[cut][key] = to;
        }
    }
}

/// The field of a written line recording the synced rows its writer wrote no line for (merged
/// into another by [`flatten_uncaptured_calls`], or with nothing the format can carry), each with
/// the line it went into: `"atuinMerged": {"<row>": "<line>"}`, the line being the row's nearest
/// written ancestor (Claude Code, pi; `""` for a row before any line, which a row hanging from it
/// then hangs from too), or the written row before it (Codex, whose rows are placed by number).
///
/// Capture reads a line's id, never this, so it pushes nothing new; the harnesses ignore a field
/// they don't know. It is what lets a transcript written from sync say which synced rows it holds
/// beyond its lines' own ids. Recorded by the writer itself, it holds however the transcript is
/// written after (the harness extending it): lines are only ever added.
pub const MERGED_FIELD: &str = "atuinMerged";

/// Record `merged` (row id, the line id it went into) on `lines`, the JSON lines a writer wrote,
/// whose own ids `id_of` reads: each on the line it went into when that is among them, else on
/// the first line (it went into a line the transcript already held).
pub fn record_merged(
    lines: &mut [serde_json::Value],
    merged: &[(String, String)],
    id_of: impl Fn(&serde_json::Value) -> Option<String>,
) {
    let ids: Vec<Option<String>> = lines.iter().map(&id_of).collect();
    for (row, into) in merged {
        let at = ids.iter().position(|id| id.as_deref() == Some(into.as_str())).unwrap_or(0);
        let Some(line) = lines.get_mut(at).and_then(serde_json::Value::as_object_mut) else {
            continue;
        };
        let field = line
            .entry(MERGED_FIELD)
            .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
        if let Some(field) = field.as_object_mut() {
            field.insert(row.clone(), serde_json::Value::String(into.clone()));
        }
    }
}

/// Helpers the writers' tests share.
#[cfg(test)]
pub(crate) mod testing {
    use std::collections::HashMap;

    use super::*;
    use crate::harnesstools::session::{ToolResult, ToolUse};

    /// `content` of a `role` row as capture syncs it with `ai.capture_tools` off (the daemon's
    /// `sanitize`): calls without their input, results without their output, no patches,
    /// reasoning as a marker, text only of what the user or the model said, nothing kept raw.
    pub fn sanitize(role: &Role, content: &[Content]) -> Vec<Content> {
        sanitize_as(role, content, false)
    }

    /// [`sanitize`], keeping tool inputs, outputs and patches when `tools` (`ai.capture_tools`
    /// on, the default).
    pub fn sanitize_as(role: &Role, content: &[Content], tools: bool) -> Vec<Content> {
        let conversation = matches!(role, Role::User | Role::Assistant);
        content
            .iter()
            .filter_map(|c| match c {
                Content::Text(_) if conversation => Some(c.clone()),
                Content::ToolUse(_) | Content::ToolResult(_) | Content::Patch(_) if tools => {
                    Some(c.clone())
                }
                Content::ToolUse(call) => Some(Content::ToolUse(ToolUse {
                    input: serde_json::Value::Null,
                    ..call.clone()
                })),
                Content::ToolResult(result) => Some(Content::ToolResult(ToolResult {
                    output: serde_json::Value::Null,
                    ..result.clone()
                })),
                Content::Reasoning(_) => Some(Content::ReasoningSummary { tokens: None }),
                Content::Summary(_) | Content::Error(_) | Content::ReasoningSummary { .. } => {
                    Some(c.clone())
                }
                Content::Text(_) | Content::Other(_) | Content::Patch(_) => None,
            })
            .collect()
    }

    /// `rows` as capture syncs them (see [`sanitize`]).
    pub fn synced(rows: Vec<RehydrateMessage>) -> Vec<RehydrateMessage> {
        synced_as(rows, false)
    }

    /// `rows` as capture syncs them, keeping tool payloads when `tools` (see [`sanitize_as`]).
    pub fn synced_as(mut rows: Vec<RehydrateMessage>, tools: bool) -> Vec<RehydrateMessage> {
        for row in &mut rows {
            row.content = sanitize_as(&row.role, &row.content, tools);
        }
        rows
    }

    /// How many calls `rows` hold without their input.
    pub fn uncaptured(rows: &[RehydrateMessage]) -> usize {
        rows.iter().flat_map(|m| &m.content).filter(|c| is_uncaptured(c)).count()
    }

    /// Re-capturing a transcript written from `synced` pushes nothing: every row it reads back
    /// with (`again`) is under an id synced already, and no id reads back more often than it
    /// was synced (once, but for fixtures whose ids were redacted). The capture sink keys its
    /// dedup on the id alone, whatever the content.
    pub fn assert_nothing_new<'a>(
        synced: &[RehydrateMessage],
        again: impl IntoIterator<Item = &'a str>,
    ) {
        let mut known: HashMap<&str, usize> = HashMap::new();
        for m in synced {
            *known.entry(m.source_id.as_str()).or_default() += 1;
        }
        for id in again {
            let left = known.get_mut(id).unwrap_or_else(|| panic!("{id} reads back unsynced"));
            assert!(*left > 0, "{id} reads back more often than it was synced");
            *left -= 1;
        }
    }
}

#[cfg(test)]
mod tests;
