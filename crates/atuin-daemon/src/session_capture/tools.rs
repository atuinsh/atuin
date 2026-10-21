//! What capture keeps of a tool call's input and output (`threads.capture_tools`).

use std::ops::Range;
use std::sync::LazyLock;

use atuin_common::harnesstools::session::{FilePatch, Patch};
use regex::Regex;
use serde_json::{Map, Value};

use super::policy::CapturePolicy;

/// The most of one tool call's input, or of its output, capture keeps: bytes of it as stored
/// (its JSON, strings escaped), past which strings are clipped, keeping their start and end, and
/// lists lose their last items. Large enough for nearly every result whole (the harnesses clip
/// their own tools' output well below it), small enough that a tool reading a whole file or
/// dumping a log can't make a huge record.
const TOOL_PAYLOAD_LIMIT: usize = 64 * 1024;

/// Strings this short are never clipped, whatever is left of the budget: they are names, kinds
/// and paths (a block's `"type": "text"`), and clipped would no longer say what they did.
const SHORT: usize = 256;

/// How far past [`TOOL_PAYLOAD_LIMIT`] the strings and items `shape` always keeps can take a
/// payload.
const OVERSHOOT: usize = 8 * SHORT;

/// How many JSON texts deep `shape` reads JSON text as the JSON it is (a Codex call's arguments,
/// a JSON file `cat` in them); deeper, it is withheld, as text hiding what it holds from
/// redaction (its quotes escaped). Each level is shaped again until it fits, so past a level or
/// two the work multiplies for nothing a harness writes.
const JSON_TEXT_DEPTH: usize = 2;

/// How many times `shape` shapes JSON text again to fit it in its room before withholding it.
const JSON_TEXT_ATTEMPTS: usize = 4;

/// Bytes past each cut a long text is redacted with, and then cut without: a secret across a
/// cut is redacted whole, rather than its part on the kept side left behind unrecognised. Longer
/// than any one secret's text (a 4096-bit RSA key in PEM is about 3.2 KiB).
const SECRET_MARGIN: usize = 4 * 1024;

/// What capture keeps of a tool call.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ToolCapture {
    /// The tool's name, and whether it failed: never its input or output, which are stored as
    /// `null` (uncaptured).
    Names,
    /// Its input and output too, with secrets redacted, media left out, and each held to
    /// 64 KiB.
    #[default]
    Payloads,
}

impl ToolCapture {
    #[must_use]
    pub const fn from_setting(capture_tools: bool) -> Self {
        if capture_tools {
            Self::Payloads
        } else {
            Self::Names
        }
    }

    /// Applies this policy to a tool call's input or output, redacting as `policy` does.
    pub(super) fn apply(self, payload: &mut Value, policy: &CapturePolicy) {
        match self {
            Self::Names => *payload = Value::Null,
            Self::Payloads => {
                let mut budget = TOOL_PAYLOAD_LIMIT;
                shape(payload, &mut budget, policy, 0);
                // `shape` keeps the payload's shape, so a harness reads it back as the same kind
                // of input or output, and holds it to the limit (give or take the short strings
                // and first items it always keeps), but for an object of very many keys, which it
                // never drops: rather than a record past the limit, or a shape no harness takes,
                // that payload is uncaptured, as every harness's writer expects some to be.
                if payload.to_string().len() > TOOL_PAYLOAD_LIMIT + OVERSHOOT {
                    *payload = Value::Null;
                }
            }
        }
    }
}

impl ToolCapture {
    /// Applies this policy to the changes a call made to files, returning whether any are kept:
    /// none with [`Self::Names`], else every file, its lines redacted as `policy` does. A file
    /// whose redaction would join or split its lines, or takes too long, and every file once
    /// their lines are past [`TOOL_PAYLOAD_LIMIT`], are kept as the file without its hunks.
    pub(super) fn apply_patch(self, patch: &mut Patch, policy: &CapturePolicy) -> bool {
        if self == Self::Names {
            return false;
        }
        let lines = patch.files.iter().flat_map(|f| &f.hunks).flat_map(|h| &h.lines);
        if lines.map(|l| l.len() + 1).sum::<usize>() > TOOL_PAYLOAD_LIMIT {
            for file in &mut patch.files {
                file.hunks.clear();
            }
        }
        for file in &mut patch.files {
            redact_hunks(file, policy);
        }
        true
    }
}

/// `file`'s hunks with their lines redacted as the text they make up together, without their
/// prefixes (` `, `-`, `+`), which would run into a secret a line starts with; a file whose
/// redaction joins or splits lines, or takes too long, is kept without its hunks, which would no
/// longer add up or might hold a secret.
fn redact_hunks(file: &mut FilePatch, policy: &CapturePolicy) {
    for hunk in &mut file.hunks {
        let split: Vec<(&str, &str)> = hunk
            .lines
            .iter()
            .map(|line| line.split_at(line.chars().next().map_or(0, char::len_utf8)))
            .collect();
        let text = split.iter().map(|(_, text)| *text).collect::<Vec<_>>().join("\n");
        let redacted = match policy.redact(&text) {
            Some(std::borrow::Cow::Borrowed(_)) => continue,
            Some(std::borrow::Cow::Owned(redacted)) => redacted,
            None => {
                file.hunks.clear();
                return;
            }
        };
        let texts: Vec<&str> = redacted.split('\n').collect();
        if texts.len() != split.len() {
            file.hunks.clear();
            return;
        }
        hunk.lines =
            split.iter().zip(texts).map(|((prefix, _), text)| format!("{prefix}{text}")).collect();
    }
}

/// Whether a call with `input` runs a command whose output may carry a credential (`atuin key`,
/// `atuin login`, ...), the output of which capture never keeps, as shell output capture never
/// does: any of its strings as a command line, a list of them (an argv) joined as one, and JSON
/// text (a Codex function call's arguments) as the JSON it is.
pub(super) fn runs_output_unsafe(input: &Value) -> bool {
    match input {
        Value::String(text) => {
            atuin_common::secrets::output_unsafe(text)
                || json_text(text).is_some_and(|parsed| runs_output_unsafe(&parsed))
        }
        Value::Array(items) => {
            let argv: Option<Vec<&str>> = items.iter().map(Value::as_str).collect();
            argv.is_some_and(|argv| atuin_common::secrets::output_unsafe(&argv.join(" ")))
                || items.iter().any(runs_output_unsafe)
        }
        Value::Object(object) => object.values().any(runs_output_unsafe),
        Value::Null | Value::Bool(_) | Value::Number(_) => false,
    }
}

/// `value` with every string redacted and, once `budget` bytes of it (as stored) are kept,
/// clipped (but never one [`SHORT`] enough to be a name), and the items of a list past it
/// dropped (but never its first); media blocks become a note. Its shape stays as it was, so a
/// harness reads it back as the same kind of input or output. `depth` is how many JSON texts it
/// is in (see [`JSON_TEXT_DEPTH`]).
fn shape(value: &mut Value, budget: &mut usize, policy: &CapturePolicy, depth: usize) {
    match value {
        Value::Object(object) => match media_note(object) {
            Some(note) => {
                *budget = budget.saturating_sub(note.to_string().len());
                *value = note;
            }
            None => {
                // A pair naming its own value (`{"name": "AWS_SECRET_ACCESS_KEY", "value": ...}`,
                // as Kubernetes, Docker and GitHub list variables) is an assignment too.
                let named = ["name", "key", "Name", "Key"]
                    .iter()
                    .find_map(|k| object.get(*k)?.as_str())
                    .map(str::to_owned);
                if let Some(name) = named {
                    for key in ["value", "Value"] {
                        if let Some(Value::String(text)) = object.get_mut(key) {
                            redact_named(&name, text, policy);
                        }
                    }
                }
                *budget = budget.saturating_sub(2);
                for (key, value) in object.iter_mut() {
                    *budget = budget.saturating_sub(stored_len(key) + 2);
                    if let Value::String(text) = value {
                        redact_named(key, text, policy);
                    }
                    shape(value, budget, policy, depth);
                }
            }
        },
        Value::Array(items) => {
            *budget = budget.saturating_sub(2);
            let mut kept = 0;
            for item in items.iter_mut() {
                if *budget == 0 && kept > 0 {
                    break;
                }
                shape(item, budget, policy, depth);
                kept += 1;
            }
            items.truncate(kept);
        }
        Value::String(text) => {
            let json = if depth < JSON_TEXT_DEPTH {
                shape_json_text(text, *budget, policy, depth)
            } else {
                json_text(text).map(|_| NOT_REDACTED.to_owned())
            };
            *text = json.unwrap_or_else(|| fit_redacted(text, (*budget).max(SHORT), policy));
            *budget = budget.saturating_sub(stored_len(text) + 1);
        }
        Value::Bool(_) | Value::Number(_) | Value::Null => {
            *budget = budget.saturating_sub(value.to_string().len() + 1);
        }
    }
}

/// JSON text (a Codex function call's arguments, a legacy Codex output), shaped as the JSON it
/// is so it stays JSON: as text, a quoted value's quotes are escaped, which hides it from
/// redaction, and clipping would cut it mid-string. Kept as written when nothing in it changes;
/// [`NOT_REDACTED`] when it doesn't fit in its room within [`JSON_TEXT_ATTEMPTS`]; `None` when
/// `text` isn't a JSON object or list. `depth` is how many JSON texts `text` is in.
fn shape_json_text(
    text: &str,
    budget: usize,
    policy: &CapturePolicy,
    depth: usize,
) -> Option<String> {
    let parsed = json_text(text)?;
    let room = budget.max(SHORT);
    let mut allowance = budget;
    for _ in 0..JSON_TEXT_ATTEMPTS {
        let mut shaped = parsed.clone();
        shape(&mut shaped, &mut allowance.clone(), policy, depth + 1);
        let shaped = if shaped == parsed {
            text.to_owned()
        } else {
            shaped.to_string()
        };
        // Escaped again as the string it is stored as, it can still be past the room: shaped
        // again, with proportionally less.
        let size = stored_len(&shaped);
        if size <= room || allowance == 0 {
            return Some(shaped);
        }
        allowance = (allowance * room / size).min(allowance - allowance.div_ceil(8));
    }
    Some(NOT_REDACTED.to_owned())
}

/// `text` parsed, when it is a JSON object or list.
fn json_text(text: &str) -> Option<Value> {
    if !text.trim_start().starts_with(['{', '[']) {
        return None;
    }
    serde_json::from_str(text).ok().filter(|v: &Value| v.is_object() || v.is_array())
}

/// The bytes `text` takes stored, as a JSON string: quoted, and escaped.
fn stored_len(text: &str) -> usize {
    serde_json::to_string(text).map_or(text.len(), |json| json.len())
}

/// `text` clipped until it takes no more than `room` bytes stored (escaping can make it take
/// several times its length), unless [`SHORT`].
fn fit(text: &str, room: usize) -> String {
    if text.len() <= SHORT {
        return text.to_owned();
    }
    fit_with(room, |keep| Some(clip(text, keep))).expect("clipping never fails")
}

/// What `clip` keeps of about `keep` bytes, for the most `keep` whose result takes no more than
/// `room` bytes stored; `None` if `clip` fails.
fn fit_with(room: usize, clip: impl Fn(usize) -> Option<String>) -> Option<String> {
    let mut keep = room;
    loop {
        let kept = clip(keep)?;
        let size = stored_len(&kept);
        if size <= room || keep <= SHORT / 4 {
            return Some(kept);
        }
        keep = (keep * room / size).min(keep - keep.div_ceil(8));
    }
}

/// `text` redacted as `policy` does and [`fit`] to `room`, or [`NOT_REDACTED`] when redacting
/// takes too long.
///
/// A text longer than could be kept is cut first, so it takes no longer to redact than one that
/// fits: only its start and end are redacted (see [`redacted_start`]), and the note between them
/// is never part of what is redacted. The user's own patterns, which can match across a cut, are
/// redacted in all of it first.
fn fit_redacted(text: &str, room: usize, policy: &CapturePolicy) -> String {
    let fitted = if text.len() <= room + NOTE + 2 * SECRET_MARGIN {
        policy.redact(text).map(|redacted| fit(&redacted, room))
    } else {
        policy.redact_users(text).and_then(|text| fit_clipped(&text, room, policy))
    };
    fitted.unwrap_or_else(|| NOT_REDACTED.to_owned())
}

/// [`fit`] of `text` redacted, redacting only its start and end, once (see [`redacted_start`]);
/// `None` when redacting takes too long.
fn fit_clipped(text: &str, room: usize, policy: &CapturePolicy) -> Option<String> {
    let keys = private_keys(text);
    let (head, head_of) = redacted_start(text, room / 2, policy)?;
    let (tail, tail_of) = redacted_end(text, &keys, room - room / 2, policy)?;
    if head_of + tail_of >= text.len() {
        // Redaction took out most of it: there's nothing between them to leave out.
        return policy.redact(text).map(|redacted| fit(&redacted, room));
    }
    let (head, tail) = ((head, head_of), (tail, tail_of));
    fit_with(room, |keep| {
        let (head, head_of) = trimmed_start(text, &head, keep / 2, policy)?;
        let (tail, tail_of) = trimmed_end(text, &keys, &tail, keep - keep / 2, policy)?;
        Some(noted(&head, text.len().saturating_sub(head_of + tail_of), &tail))
    })
}

/// `start`, a [`redacted_start`] of `text`, cut to about `want` bytes, and how many bytes of
/// `text` that stands for: cut within, when what it cuts off is as written (so stands for as many
/// bytes of `text`), else redacted again for `want`.
fn trimmed_start(
    text: &str,
    (start, of): &(String, usize),
    want: usize,
    policy: &CapturePolicy,
) -> Option<(String, usize)> {
    let cut = start.floor_char_boundary(want);
    if start.len() - cut <= common_suffix(&text[..*of], start) {
        return Some((start[..cut].to_owned(), of - (start.len() - cut)));
    }
    redacted_start(text, want, policy)
}

/// [`trimmed_start`] for a [`redacted_end`] of `text`.
fn trimmed_end(
    text: &str,
    keys: &[Range<usize>],
    (end, of): &(String, usize),
    want: usize,
    policy: &CapturePolicy,
) -> Option<(String, usize)> {
    let cut = end.ceil_char_boundary(end.len().saturating_sub(want));
    if cut <= common_prefix(&text[text.len() - of..], end) {
        return Some((end[cut..].to_owned(), of - cut));
    }
    redacted_end(text, keys, want, policy)
}

/// How many times [`redacted_start`] reads further for a start redaction shrank.
const REDACTED_ATTEMPTS: usize = 4;

/// How many times the bytes it is to hold [`redacted_start`] reads at most: a start redaction
/// shrank further holds less, rather than taking longer to redact than a capture may.
const REDACTED_GROWTH: usize = 8;

/// The start of `text`, redacted, about `want` bytes long, and how many bytes of `text` it stands
/// for (no more than half of it); `None` when redacting takes too long.
///
/// It is redacted with [`SECRET_MARGIN`] more past its cut, which is then cut off, so a secret
/// across the cut is redacted whole rather than its part on the kept side left behind
/// unrecognised: a secret across the end of what is redacted is unseen, so past the last
/// redaction, where it would be, what is cut off is all as written, mapping the cut back to the
/// text exactly. A start that redaction shrank is read further, so it still holds about `want`.
fn redacted_start(text: &str, want: usize, policy: &CapturePolicy) -> Option<(String, usize)> {
    let limit = text.len() / 2;
    let mut cut = text.floor_char_boundary(want.min(limit));
    for attempt in 1..=REDACTED_ATTEMPTS {
        let window = &text[..text.floor_char_boundary(cut + SECRET_MARGIN)];
        let redacted = policy.redact(window)?;
        let same = common_suffix(window, &redacted);
        let drop = same.min(window.len() - cut);
        let (kept, of) = (&redacted[..redacted.len() - drop], window.len() - drop);
        if kept.len() >= want || cut >= limit || attempt == REDACTED_ATTEMPTS {
            return Some((kept.to_owned(), of));
        }
        let more = (cut * want / kept.len().max(1)).min(want * REDACTED_GROWTH);
        cut = text.floor_char_boundary(more.min(limit));
    }
    unreachable!("the last attempt returns")
}

/// [`redacted_start`] for the end of `text`. A private key it would hold without its start line
/// (which redaction finds a key by) is left out, up to its end line, or the end of `text` for a
/// key cut off before it; `keys` are `text`'s [`private_keys`].
fn redacted_end(
    text: &str,
    keys: &[Range<usize>],
    want: usize,
    policy: &CapturePolicy,
) -> Option<(String, usize)> {
    let limit = text.len() - text.len() / 2;
    let mut cut = text.ceil_char_boundary(text.len() - want.min(limit));
    for attempt in 1..=REDACTED_ATTEMPTS {
        let mut from = text.ceil_char_boundary(cut.saturating_sub(SECRET_MARGIN));
        if let Some(key) = keys.iter().find(|key| key.start < from && key.end > from) {
            from = key.end;
            cut = cut.max(from);
        }
        let window = &text[from..];
        let redacted = policy.redact(window)?;
        let same = common_prefix(window, &redacted);
        let drop = same.min(cut - from);
        let (kept, of) = (&redacted[drop..], window.len() - drop);
        if kept.len() >= want || text.len() - cut >= limit || attempt == REDACTED_ATTEMPTS {
            return Some((kept.to_owned(), of));
        }
        let more = ((text.len() - cut) * want / kept.len().max(1)).min(want * REDACTED_GROWTH);
        cut = text.ceil_char_boundary(text.len() - more.min(limit));
    }
    unreachable!("the last attempt returns")
}

/// Where the private keys in `text` are, as redaction finds them: from their start line past
/// their end line, or to the end of `text` for a key cut off before it.
fn private_keys(text: &str) -> Vec<Range<usize>> {
    static KEY: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"-----BEGIN[ A-Z0-9_-]{0,100}PRIVATE KEY(?: BLOCK)?-----(?s:.+?)(?:-----END[ A-Z0-9_-]{0,100}PRIVATE KEY(?: BLOCK)?-----|\z)",
        )
        .expect("valid")
    });
    KEY.find_iter(text).map(|key| key.range()).collect()
}

/// How many bytes `a` and `b` start with alike, on a character boundary.
fn common_prefix(a: &str, b: &str) -> usize {
    let mut n = a.bytes().zip(b.bytes()).take_while(|(x, y)| x == y).count();
    while !a.is_char_boundary(n) {
        n -= 1;
    }
    n
}

/// How many bytes `a` and `b` end with alike, on a character boundary.
fn common_suffix(a: &str, b: &str) -> usize {
    let mut n = a.bytes().rev().zip(b.bytes().rev()).take_while(|(x, y)| x == y).count();
    while !a.is_char_boundary(a.len() - n) {
        n -= 1;
    }
    n
}

/// `text`, the value of `key`, redacted as the assignment it is: a secret is often known by the
/// name it is kept under (`{"AWS_SECRET_ACCESS_KEY": "..."}`), which the value alone doesn't say.
///
/// Only a value's start follows its name: of a long one, as much as capture could keep is
/// redacted so, and the rest dropped should that redact anything, as it may go on with what was.
/// Shaped, it is all redacted as any text is.
fn redact_named(key: &str, text: &mut String, policy: &CapturePolicy) {
    let line = format!("{key}={}", &text[..text.floor_char_boundary(TOOL_PAYLOAD_LIMIT)]);
    let Some(redacted) = policy.redact(&line) else {
        NOT_REDACTED.clone_into(text);
        return;
    };
    if redacted != line {
        let value = redacted.strip_prefix(key).and_then(|rest| rest.strip_prefix('='));
        value.unwrap_or(atuin_common::secrets::REDACTED).clone_into(text);
    }
}

/// A note in place of an image or document block carrying its bytes (base64, which clipped would
/// no longer be), in the text block of the same family: Codex's `input_*` items, else the `text`
/// blocks Claude Code, opencode and pi use. A block that only names or describes one (a listing's
/// `{"type": "file", "name": ...}`) is kept.
fn media_note(object: &Map<String, Value>) -> Option<Value> {
    let kind = object.get("type")?.as_str()?;
    if !matches!(kind, "image" | "input_image" | "document" | "file" | "input_file") {
        return None;
    }
    let inline = object.get("source").is_some_and(Value::is_object)
        || ["data", "file_data"].iter().any(|k| object.get(*k).is_some_and(Value::is_string))
        || ["image_url", "url"].iter().any(|k| {
            object.get(*k).and_then(Value::as_str).is_some_and(|url| url.starts_with("data:"))
        });
    if !inline {
        return None;
    }
    let text = Value::String(format!("[{kind} not captured]"));
    let kind = if kind.starts_with("input_") {
        "input_text"
    } else {
        "text"
    };
    Some(Value::Object(Map::from_iter([
        ("type".to_owned(), Value::String(kind.to_owned())),
        ("text".to_owned(), text),
    ])))
}

/// In place of a string that took too long to redact (see
/// [`REDACT_BUDGET`](atuin_common::secrets::REDACT_BUDGET)), or JSON text too deep or too big to
/// shape as JSON (see [`JSON_TEXT_DEPTH`]).
pub(super) const NOT_REDACTED: &str = "[not captured: took too long to redact]";

/// In place of what a call writes to a file holding credentials.
const WITHHELD: &str = "[not captured: a file holding credentials]";

/// Whether `input` is what [`withhold_written`] leaves of one: it holds [`WITHHELD`], or is a
/// patch's headers alone. Judged again, it may no longer say what it wrote (a Codex script left
/// as [`WITHHELD`] makes no call).
pub(super) fn is_withheld(input: &Value) -> bool {
    match input {
        Value::String(text) => {
            text.contains(WITHHELD)
                || (!text.trim().is_empty()
                    && text.lines().all(|line| line.trim_start().starts_with("*** ")))
        }
        Value::Array(items) => items.iter().any(is_withheld),
        Value::Object(object) => object.values().any(is_withheld),
        Value::Null | Value::Bool(_) | Value::Number(_) => false,
    }
}

/// `input`, a call writing to a file holding credentials, without what it writes: every string
/// but the paths it names, and of a patch, all but its headers (`*** Update File: .env`). It keeps
/// its shape, so a harness reads it back as the same call.
pub(super) fn withhold_written(input: &mut Value) {
    fn blank(value: &mut Value) {
        match value {
            Value::String(text) => WITHHELD.clone_into(text),
            Value::Array(items) => items.iter_mut().for_each(blank),
            Value::Object(object) => object.values_mut().for_each(blank),
            Value::Null | Value::Bool(_) | Value::Number(_) => {}
        }
    }
    match input {
        Value::Object(object) => {
            for (key, value) in object.iter_mut() {
                if !PATH_KEYS.contains(&key.as_str()) {
                    blank(value);
                }
            }
        }
        Value::String(text) => match json_text(text) {
            Some(mut parsed) => {
                withhold_written(&mut parsed);
                *text = parsed.to_string();
            }
            None => {
                let headers: Vec<&str> =
                    text.lines().filter(|line| line.trim_start().starts_with("*** ")).collect();
                *text = if headers.is_empty() {
                    WITHHELD.to_owned()
                } else {
                    headers.join("\n")
                };
            }
        },
        Value::Array(_) | Value::Null | Value::Bool(_) | Value::Number(_) => blank(input),
    }
}

/// The keys a call names a file under, kept when what it writes isn't.
const PATH_KEYS: &[&str] =
    &["file_path", "filePath", "path", "notebook_path", "filename", "file", "paths", "file_paths"];

/// About the length of [`clip`]'s note.
const NOTE: usize = 40;

/// `text` cut to about `keep` bytes, its start and end kept around a note of how much was left
/// out.
fn clip(text: &str, keep: usize) -> String {
    // Within a note's length of `keep`, the note would leave it no shorter.
    if text.len() <= keep + NOTE {
        return text.to_owned();
    }
    let head = &text[..text.floor_char_boundary(keep / 2)];
    let tail = &text[text.ceil_char_boundary(text.len() - (keep - keep / 2))..];
    noted(head, text.len() - head.len() - tail.len(), tail)
}

/// `head` and `tail` around a note that `omitted` bytes between them were left out.
fn noted(head: &str, omitted: usize, tail: &str) -> String {
    format!("{head}\n[… {omitted} bytes not captured …]\n{tail}")
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use rstest::rstest;
    use serde_json::json;

    use super::*;

    fn kept(mut value: Value) -> Value {
        ToolCapture::Payloads.apply(&mut value, &CapturePolicy::default());
        value
    }

    #[rstest]
    #[case::object(json!({"command": "ls -la", "timeout": 5}))]
    #[case::string(json!("total 0\n"))]
    #[case::blocks(json!([{"type": "text", "text": "ok"}]))]
    #[case::empty(json!(""))]
    fn a_small_payload_is_kept_as_it_is(#[case] value: Value) {
        assert_eq!(kept(value.clone()), value);
    }

    #[rstest]
    #[case::input(json!({"command": "export AWS_SECRET_ACCESS_KEY=PRIVATEKEY"}))]
    #[case::output(json!("AWS_SECRET_ACCESS_KEY=PRIVATEKEY\n"))]
    #[case::arguments(json!(r#"{"cmd":"AWS_SECRET_ACCESS_KEY=PRIVATEKEY"}"#))]
    #[case::named(json!({"env": {"AWS_SECRET_ACCESS_KEY": "PRIVATEKEY"}}))]
    #[case::name_value(json!({"env": [{"name": "AWS_SECRET_ACCESS_KEY", "value": "PRIVATEKEY"}]}))]
    #[case::named_in_text(json!("{\"AWS_SECRET_ACCESS_KEY\": \"PRIVATEKEY\"}"))]
    #[case::quoted_in_arguments(json!(r#"{"cmd":"export AWS_SECRET_ACCESS_KEY=\"PRIVATEKEY\" && aws s3 ls"}"#))]
    #[case::single_quoted_in_arguments(json!(r#"{"cmd":"export AWS_SECRET_ACCESS_KEY='PRIVATEKEY'","workdir":"/w"}"#))]
    #[case::argv_in_arguments(json!(r#"{"command":["bash","-lc","AWS_SECRET_ACCESS_KEY=\"PRIVATEKEY\" aws s3 ls"]}"#))]
    fn secrets_are_redacted(#[case] value: Value) {
        let kept = kept(value).to_string();
        assert!(!kept.contains("PRIVATEKEY"), "{kept}");
        assert!(kept.contains("AWS_SECRET_ACCESS_KEY"), "the name stays: {kept}");
    }

    /// A patch a call makes (Codex `apply_patch`'s text, opencode's `patchText`) has its lines'
    /// secrets redacted, what it adds as well as what it removes.
    #[rstest]
    #[case::codex(json!("*** Begin Patch\n*** Add File: db.yml\n+production:\n+  password: hunter2xyz\n*** End Patch\n"))]
    #[case::opencode(json!({"patchText": "*** Begin Patch\n*** Update File: app.properties\n@@\n-db.password=hunter1xyz\n+db.password=hunter2xyz\n*** End Patch"}))]
    fn a_patchs_added_secrets_are_redacted(#[case] value: Value) {
        let kept = kept(value).to_string();
        assert!(!kept.contains("hunter"), "{kept}");
        assert!(kept.contains("password"), "the name stays: {kept}");
    }

    #[rstest]
    fn names_only_keeps_nothing() {
        let mut value = json!({"command": "ls"});
        ToolCapture::Names.apply(&mut value, &CapturePolicy::default());
        assert!(value.is_null());
    }

    #[rstest]
    #[case::claude(
        json!({"type": "image", "source": {"type": "base64", "data": "iVBORw0KGgo"}}),
        json!({"type": "text", "text": "[image not captured]"}),
    )]
    #[case::codex(
        json!({"type": "input_image", "image_url": "data:image/png;base64,iVBORw0KGgo"}),
        json!({"type": "input_text", "text": "[input_image not captured]"}),
    )]
    #[case::text(json!({"type": "text", "text": "hi"}), json!({"type": "text", "text": "hi"}))]
    #[case::listing(json!({"type": "file", "name": "a.rs"}), json!({"type": "file", "name": "a.rs"}))]
    fn media_becomes_a_note(#[case] value: Value, #[case] want: Value) {
        assert_eq!(kept(json!([value])), json!([want]));
    }

    #[rstest]
    fn a_long_output_keeps_its_start_and_end() {
        let text = format!("START{}END", "x".repeat(TOOL_PAYLOAD_LIMIT * 4));
        let Value::String(kept) = kept(json!(text)) else {
            panic!("still a string")
        };
        assert!(kept.starts_with("START") && kept.ends_with("END"));
        assert!(kept.contains("bytes not captured"));
        assert!(kept.len() < TOOL_PAYLOAD_LIMIT + 64);
    }

    /// A long output is clipped before it is redacted, so one far too long to redact whole in
    /// time (this one takes seconds) is still kept, its start and end redacted.
    #[rstest]
    fn a_long_output_is_clipped_not_dropped_for_taking_long_to_redact() {
        let token = "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";
        let text = format!("{token} {}{token}", "atuin login ".repeat(1 << 20));
        let Value::String(kept) = kept(json!(text)) else {
            panic!("still a string")
        };
        assert!(kept.contains("bytes not captured"), "{kept}");
        assert!(kept.starts_with("**** atuin login") && kept.ends_with("atuin login ****"));
    }

    /// Wherever a long output is cut, no part of a secret across the cut is left behind.
    #[rstest]
    fn a_secret_across_a_cut_is_redacted_whole(#[values(0, 7, 20, 33)] shift: usize) {
        let token = "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";
        let text = format!("{}{}", " ".repeat(shift), format!("{token} ").repeat(100_000));
        let Value::String(kept) = kept(json!(text)) else {
            panic!("still a string")
        };
        assert!(kept.contains("bytes not captured"));
        for part in kept.split([' ', '\n']).filter(|part| !part.is_empty()) {
            assert!(
                part.chars().all(|c| c == '*') || !token.contains(part) || part.len() < 4,
                "{part:?} is part of a secret"
            );
        }
    }

    /// A private key longer than the margin a cut is redacted with, across the cut (an armored
    /// PGP secret key is 5-12 KiB): none of it is kept, though the start line redaction finds it
    /// by is on the other side.
    #[rstest]
    fn a_long_private_key_across_a_cut_is_not_kept(#[values(false, true)] at_the_end: bool) {
        let line =
            |i: usize| format!("lQdGBGVxyz{i:03}").repeat(6).chars().take(64).collect::<String>();
        let body: String = (0..140).map(|i| line(i) + "\n").collect();
        let key = format!(
            "-----BEGIN PGP PRIVATE KEY BLOCK-----\n\n{body}=AbCd\n-----END PGP PRIVATE KEY \
             BLOCK-----\n"
        );
        assert!(key.len() > 2 * SECRET_MARGIN);
        // Spaces, not newlines, which take twice the room stored and move the cuts.
        let (near, far) = ("x ".repeat(13_000), "log line of output ".repeat(6000));
        let text = if at_the_end {
            format!("{far}{key}{near}")
        } else {
            format!("{near}{key}{far}")
        };
        let Value::String(kept) = kept(json!(text)) else {
            panic!("still a string")
        };
        assert!(kept.contains("bytes not captured"));
        assert!(!kept.contains("lQdGBG"), "{kept}");
    }

    /// A match of the user's own patterns across a cut, whose start or end is in what is cut
    /// out, still redacts what it covers of what is kept.
    #[rstest]
    #[case::into_the_end(100, 180, 190)]
    #[case::out_of_the_start(10, 20, 60)]
    fn a_users_pattern_across_a_cut_is_redacted(
        #[case] open: usize,
        #[case] secret: usize,
        #[case] close: usize,
    ) {
        let policy = CapturePolicy::with_redact_patterns(vec![
            Regex::new(r"(?s)<secret>.*?</secret>").unwrap(),
        ]);
        let mut text = "x ".repeat(100 * 1024);
        for (at, part) in [(close, "</secret>"), (secret, "violet-7-meadow"), (open, "<secret>")] {
            text.insert_str(at * 1024, part);
        }
        let mut value = json!(text);
        ToolCapture::Payloads.apply(&mut value, &policy);
        let kept = value.to_string();
        assert!(kept.contains("bytes not captured"));
        assert!(!kept.contains("violet-7-meadow"), "{kept}");
    }

    /// A private key's armored body, `lines` lines of it, without its end line, as output cut off
    /// mid-key leaves it.
    fn unended_key(lines: usize) -> String {
        let line =
            |i: usize| format!("lQdGBGVxyz{i:03}").repeat(6).chars().take(64).collect::<String>();
        let body: String = (0..lines).map(|i| line(i) + "\n").collect();
        format!("-----BEGIN PGP PRIVATE KEY BLOCK-----\n\n{body}")
    }

    /// A private key cut off before its end line, its start line before the window the end of a
    /// clipped text is redacted in: none of it is kept, in a later item left little of the budget
    /// as in a text whose key outruns the window.
    #[rstest]
    #[case::strings(json!(["log line of output ".repeat(3340), unended_key(200)]))]
    #[case::string(json!(format!("{}{}", "log line of output ".repeat(6000), unended_key(700))))]
    #[case::then_a_key(json!(format!(
        "{}{}{}-----END PGP PRIVATE KEY BLOCK-----\nlog line of output",
        "log line of output ".repeat(6000),
        unended_key(700),
        unended_key(20),
    )))]
    fn an_unended_private_key_before_a_cut_is_not_kept(#[case] value: Value) {
        let kept = kept(value).to_string();
        assert!(kept.contains("bytes not captured"), "{kept:.200}");
        assert!(!kept.contains("lQdGBG"), "{kept}");
    }

    /// Text redaction shrinks a lot (a long run of tokens) is kept as much of as fits, and the
    /// note says exactly how much of the text, as written, it left out: also when what is kept
    /// takes more room stored than it is long (newlines, escaped), and is cut again to fit.
    #[rstest]
    #[case::fits_once_redacted(5_000)]
    #[case::does_not(200_000)]
    #[case::by_far(1_000_000)]
    fn heavily_redacted_text_is_kept_as_much_as_fits(
        #[case] tokens: usize,
        #[values(" ", "\n")] separator: &str,
    ) {
        let token = "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";
        let text = format!("{token}{separator}").repeat(tokens);
        let Value::String(kept) = kept(json!(text)) else {
            panic!("still a string")
        };
        let redacted = format!("****{separator}").repeat(tokens);
        if stored_len(&redacted) <= TOOL_PAYLOAD_LIMIT {
            assert_eq!(kept, redacted);
            return;
        }
        assert!(stored_len(&kept) > TOOL_PAYLOAD_LIMIT * 3 / 4, "kept {} bytes", kept.len());
        let (head, rest) = kept.split_once("\n[… ").unwrap();
        let (omitted, tail) = rest.split_once(" bytes not captured …]\n").unwrap();
        // What a part kept stands for, as written: each `****` a token.
        let written = |part: &str| part.len() + part.matches("****").count() * (token.len() - 4);
        let omitted: usize = omitted.parse().unwrap();
        assert_eq!(written(head) + omitted + written(tail), text.len());
    }

    /// JSON text nested in JSON text, many levels deep, is shaped as JSON only a level or two
    /// down, and as text below: shaping each level again until it fits, as deep as it goes, took
    /// half a minute (holding the daemon) for this 1.5 MB call.
    #[rstest]
    fn deeply_nested_json_text_is_shaped_in_bounded_time() {
        let keys: Map<String, Value> =
            (0..3000).map(|i| (format!("k{i:04}"), json!("v"))).collect();
        let mut text = Value::Object(keys).to_string();
        for _ in 0..6 {
            text = json!([text]).to_string();
        }
        let started = std::time::Instant::now();
        let kept = kept(json!({"command": text}));
        assert!(started.elapsed() < std::time::Duration::from_secs(10), "{:?}", started.elapsed());
        assert!(kept.to_string().len() <= TOOL_PAYLOAD_LIMIT + OVERSHOOT);
    }

    /// JSON text nested deeper than `shape` reads it as JSON is not kept as text, where its keys'
    /// quotes are escaped and hide a secret from redaction.
    #[rstest]
    fn json_text_nested_too_deep_keeps_no_secret(#[values(0, 1, 2, 3, 4)] levels: usize) {
        let mut text = json!({"client_secret": "violet-7-meadow"}).to_string();
        for _ in 0..levels {
            text = json!([text]).to_string();
        }
        let kept = kept(json!(text)).to_string();
        assert!(!kept.contains("violet-7-meadow"), "{kept}");
    }

    /// JSON text too big to shape as JSON in its room (more keys than fit, which `shape` never
    /// drops) is not kept as text either, where JSON text in it would hide a secret.
    #[rstest]
    fn json_text_too_big_to_shape_keeps_no_secret() {
        let mut keys: Map<String, Value> =
            (0..8000).map(|i| (format!("k{i:04}"), json!("v"))).collect();
        keys.insert("a".to_owned(), json!(json!({"client_secret": "violet-7-meadow"}).to_string()));
        let kept = kept(json!(Value::Object(keys).to_string())).to_string();
        assert!(!kept.contains("violet-7-meadow"), "{kept:.200}");
    }

    /// The budget is shared: once early strings spend it, later long ones are clipped to little
    /// more than their note, while short ones (the blocks' kinds) are kept.
    #[rstest]
    fn the_budget_covers_the_whole_payload() {
        let (big, late) = ("x".repeat(TOOL_PAYLOAD_LIMIT - SHORT), "y".repeat(4 * SHORT));
        let kept = kept(json!([{"type": "text", "text": big}, {"type": "text", "text": late}]));
        assert_eq!(kept[0]["text"], json!(big));
        assert_eq!(kept[1]["type"], json!("text"));
        assert!(kept[1]["text"].as_str().unwrap().len() < 2 * SHORT);
    }

    /// JSON text (a Codex function call's arguments, a `cat` of a JSON file) is kept as written,
    /// not re-formatted, when nothing in it changes.
    #[rstest]
    fn json_text_is_kept_as_written() {
        let text = "{\n  \"name\": \"x\",\n  \"n\": 12345678901234567890\n}";
        assert_eq!(kept(json!(text)), json!(text));
    }

    /// JSON text redacted or clipped is still JSON, with every key it had: a Codex writer reads
    /// a call whose arguments don't parse as a custom tool's.
    #[rstest]
    #[case::redacted(r#"{"cmd":"export AWS_SECRET_ACCESS_KEY='x'","workdir":"/w"}"#.to_owned())]
    #[case::named(r#"{"env":{"AWS_SECRET_ACCESS_KEY":"x"},"workdir":"/w"}"#.to_owned())]
    #[case::clipped(json!({"cmd": format!("cat <<'EOF'\n{}\nEOF", "x".repeat(3 * TOOL_PAYLOAD_LIMIT)), "workdir": "/w"}).to_string())]
    #[case::escaped(json!({"cmd": "\"q\"".repeat(TOOL_PAYLOAD_LIMIT / 4), "workdir": "/w"}).to_string())]
    fn json_text_stays_json(#[case] text: String) {
        let Value::String(kept) = kept(json!(text)) else {
            panic!("still a string")
        };
        let parsed: Value = serde_json::from_str(&kept).expect("still JSON");
        assert_eq!(parsed["workdir"], json!("/w"), "{kept:.200}");
        assert!(stored_len(&kept) <= TOOL_PAYLOAD_LIMIT + 64, "{}", stored_len(&kept));
    }

    /// Text that escaping makes much longer (colours, quotes, JSON in JSON) is clipped to fit as
    /// stored, and stays the text it was: never its own JSON encoding, nor anything but the
    /// shape it had.
    #[rstest]
    #[case::coloured("\u{1b}[32mok\u{1b}[0m test\n".repeat(TOOL_PAYLOAD_LIMIT / 16))]
    #[case::quoted("\"a\",\"b\"\n".repeat(TOOL_PAYLOAD_LIMIT / 6))]
    fn escaped_text_keeps_its_shape(#[case] text: String) {
        let Value::String(output) = kept(json!(text)) else {
            panic!("a string stays one")
        };
        assert_eq!(&output[..20], &text[..20]);
        assert!(stored_len(&output) <= TOOL_PAYLOAD_LIMIT + 64);

        let kept = kept(json!([{"type": "text", "text": text}, {"type": "text", "text": "late"}]));
        assert_eq!(kept[0]["type"], json!("text"), "blocks stay blocks: {:.200}", kept.to_string());
        assert!(kept[0]["text"].as_str().unwrap().starts_with(&text[..20]));
        assert!(kept.to_string().len() <= TOOL_PAYLOAD_LIMIT + 2 * SHORT);
    }

    #[rstest]
    #[case::claude(json!({"command": "atuin key"}), true)]
    #[case::sudo(json!({"command": "cd ~ && atuin login -u me"}), true)]
    #[case::codex_text(json!(r#"{"cmd":"atuin key --base64","workdir":"/w"}"#), true)]
    #[case::codex_argv(json!({"command": ["atuin", "key"]}), true)]
    #[case::bash_lc(json!(r#"{"command":["bash","-lc","atuin register"]}"#), true)]
    #[case::bash_lc_line(json!({"command": "bash -lc 'atuin key'"}), true)]
    #[case::other(json!({"command": "atuin history list"}), false)]
    #[case::mention(json!({"command": "echo 'run atuin key later'"}), false)]
    #[case::read(json!({"file_path": "/home/me/atuin/key"}), false)]
    fn calls_whose_output_may_hold_a_credential(#[case] input: Value, #[case] unsafe_: bool) {
        assert_eq!(runs_output_unsafe(&input), unsafe_);
    }

    /// A write to a credential file keeps its shape and its paths, and nothing it writes.
    #[rstest]
    #[case::claude(
        json!({"file_path": ".env", "content": "A=1"}),
        json!({"file_path": ".env", "content": WITHHELD}),
    )]
    #[case::pi_edits(
        json!({"path": ".env", "edits": [{"oldText": "A=1", "newText": "A=2"}]}),
        json!({"path": ".env", "edits": [{"oldText": WITHHELD, "newText": WITHHELD}]}),
    )]
    #[case::codex_arguments(
        json!(r#"{"path":".env","content":"A=1"}"#),
        json!(json!({"path": ".env", "content": WITHHELD}).to_string()),
    )]
    #[case::patch(
        json!("*** Begin Patch\n*** Update File: .env\n@@\n-A=1\n+A=2\n*** End Patch"),
        json!("*** Begin Patch\n*** Update File: .env\n*** End Patch"),
    )]
    fn a_credential_files_writes_are_withheld(#[case] mut input: Value, #[case] want: Value) {
        withhold_written(&mut input);
        assert_eq!(input, want);
    }

    /// A payload of many values each too short to clip is still held to the limit.
    #[rstest]
    #[case::listing(Value::Array((0..20_000).map(|n| json!({"name": format!("pkg-{n}"), "version": "1.0.0"})).collect()))]
    #[case::input(json!({"files": (0..20_000).map(|n| format!("src/{n}.rs")).collect::<Vec<_>>()}))]
    fn many_short_values_are_held_to_the_limit(#[case] value: Value) {
        let (object, array) = (value.is_object(), value.is_array());
        let kept = kept(value);
        assert_eq!(kept.is_array(), array, "a list stays a list");
        let size = kept.to_string().len();
        assert!(size <= TOOL_PAYLOAD_LIMIT + SHORT, "{size}");
        assert_eq!(kept.is_object(), object, "an input stays an object");
    }

    /// An object of more keys than fit is uncaptured: `shape` never drops a key.
    #[rstest]
    fn an_object_past_the_limit_is_uncaptured() {
        let object: Map<String, Value> =
            (0..8_000).map(|n| (format!("key-{n}"), json!(format!("value-{n}")))).collect();
        assert!(kept(Value::Object(object)).is_null());
    }

    proptest! {
        /// A payload is held to the limit (a short string's length over), whatever its strings
        /// hold, and a list stays a list.
        #[rstest]
        fn clipping_is_bounded_and_a_list_stays_one(
            strings in proptest::collection::vec(".{0,40000}", 0..4),
        ) {
            let value = Value::Array(strings.iter().cloned().map(Value::String).collect());
            let kept = kept(value);
            prop_assert!(kept.to_string().len() <= TOOL_PAYLOAD_LIMIT + OVERSHOOT);
            // A list stays one, of its first items (all of them when they fit).
            let items = kept.as_array().unwrap();
            prop_assert!(items.len() <= strings.len() && (strings.is_empty() || !items.is_empty()));
        }

        #[rstest]
        fn clip_keeps_text_that_fits_whole(text in ".{0,50}", keep in 200usize..400) {
            prop_assert_eq!(clip(&text, keep), text);
        }
    }

    fn patch(lines: &[&str]) -> Patch {
        use atuin_common::harnesstools::session::{Change, Hunk};
        let n = lines.len() as u64;
        Patch {
            call: "call_1".to_owned().into(),
            files: vec![FilePatch {
                path: "/w/.env".to_owned(),
                change: Change::Update,
                moved_to: None,
                hunks: vec![Hunk {
                    old_start: 1,
                    old_lines: n,
                    new_start: 1,
                    new_lines: n,
                    lines: lines.iter().map(|l| (*l).to_owned()).collect(),
                }],
            }],
        }
    }

    /// A patch is kept with its lines, unless only tool names are captured.
    #[rstest]
    fn a_patch_is_kept_unless_only_names_are() {
        let lines = [" a", "-b", "+c"];
        let mut kept = patch(&lines);
        assert!(ToolCapture::Payloads.apply_patch(&mut kept, &CapturePolicy::default()));
        assert_eq!(kept, patch(&lines));
        assert!(!ToolCapture::Names.apply_patch(&mut patch(&lines), &CapturePolicy::default()));
    }

    /// A secret on a line is redacted, the line keeping its side.
    #[rstest]
    fn a_patchs_secrets_are_redacted() {
        let mut kept = patch(&[" a", "-AWS_SECRET_ACCESS_KEY=old", "+AWS_SECRET_ACCESS_KEY=new"]);
        assert!(ToolCapture::Payloads.apply_patch(&mut kept, &CapturePolicy::default()));
        let lines = &kept.files[0].hunks[0].lines;
        assert_eq!(lines.len(), 3);
        assert!(lines[1].starts_with('-') && lines[2].starts_with('+'), "{lines:?}");
        assert!(!lines.iter().any(|l| l.contains("old") || l.contains("new")), "{lines:?}");
    }

    /// A token a line starts with is found without the line's prefix, which would otherwise run
    /// into it, and redacted with the line keeping its side.
    #[rstest]
    fn a_token_starting_a_line_is_redacted() {
        let token = "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";
        let mut kept = patch(&[" a", &format!("-{token}"), &format!("+{token}")]);
        assert!(ToolCapture::Payloads.apply_patch(&mut kept, &CapturePolicy::default()));
        let lines = &kept.files[0].hunks[0].lines;
        assert_eq!(lines.len(), 3);
        assert!(lines[1].starts_with('-') && lines[2].starts_with('+'), "{lines:?}");
        assert!(!lines.iter().any(|l| l.contains(token)), "{lines:?}");
    }

    /// Past the payload limit, every file is kept without its lines.
    #[rstest]
    fn a_patch_past_the_limit_keeps_only_its_files() {
        let long = format!("+{}", "x".repeat(TOOL_PAYLOAD_LIMIT));
        let mut kept = patch(&[&long]);
        assert!(ToolCapture::Payloads.apply_patch(&mut kept, &CapturePolicy::default()));
        assert_eq!(kept.files.len(), 1);
        assert!(kept.files[0].hunks.is_empty());
    }
}
