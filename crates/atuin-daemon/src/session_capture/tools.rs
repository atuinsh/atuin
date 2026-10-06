//! What capture keeps of a tool call's input and output (`ai.capture_tools`).

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
                shape(payload, &mut budget, policy);
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
/// harness reads it back as the same kind of input or output.
fn shape(value: &mut Value, budget: &mut usize, policy: &CapturePolicy) {
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
                    shape(value, budget, policy);
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
                shape(item, budget, policy);
                kept += 1;
            }
            items.truncate(kept);
        }
        Value::String(text) => {
            if let Some(shaped) = shape_json_text(text, *budget, policy) {
                *text = shaped;
            } else {
                *text = match policy.redact(text) {
                    Some(redacted) => fit(&redacted, (*budget).max(SHORT)),
                    None => NOT_REDACTED.to_owned(),
                };
            }
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
/// `None` when `text` isn't a JSON object or list.
fn shape_json_text(text: &str, budget: usize, policy: &CapturePolicy) -> Option<String> {
    let parsed = json_text(text)?;
    let room = budget.max(SHORT);
    let mut allowance = budget;
    loop {
        let mut shaped = parsed.clone();
        shape(&mut shaped, &mut allowance.clone(), policy);
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
    let mut keep = room;
    loop {
        let kept = clip(text, keep);
        let size = stored_len(&kept);
        if size <= room || keep <= SHORT / 4 {
            return kept;
        }
        keep = (keep * room / size).min(keep - keep.div_ceil(8));
    }
}

/// `text`, the value of `key`, redacted as the assignment it is: a secret is often known by the
/// name it is kept under (`{"AWS_SECRET_ACCESS_KEY": "..."}`), which the value alone doesn't say.
fn redact_named(key: &str, text: &mut String, policy: &CapturePolicy) {
    let line = format!("{key}={text}");
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
/// [`REDACT_BUDGET`](atuin_common::secrets::REDACT_BUDGET)).
pub(super) const NOT_REDACTED: &str = "[not captured: took too long to redact]";

/// In place of what a call writes to a file holding credentials.
const WITHHELD: &str = "[not captured: a file holding credentials]";

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
    let head = text.floor_char_boundary(keep / 2);
    let tail = text.ceil_char_boundary(text.len() - (keep - keep / 2));
    let omitted = tail - head;
    format!("{}\n[… {omitted} bytes not captured …]\n{}", &text[..head], &text[tail..])
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
}
