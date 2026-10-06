//! What capture keeps of a tool call's input and output (`ai.capture_tools`).

use serde_json::{Map, Value};

/// The most of one tool call's input, or of its output, capture keeps: the bytes of its strings,
/// past which they are clipped, keeping their start and end. Large enough for nearly every result
/// whole (the harnesses clip their own tools' output well below it), small enough that a tool
/// reading a whole file or dumping a log can't make a huge record.
const TOOL_PAYLOAD_LIMIT: usize = 64 * 1024;

/// Strings this short are never clipped, whatever is left of the budget: they are names, kinds
/// and paths (a block's `"type": "text"`), and clipped would no longer say what they did.
const SHORT: usize = 256;

/// What capture keeps of a tool call.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ToolCapture {
    /// The tool's name, and whether it failed: never its input or output, which are stored as
    /// `null` (uncaptured).
    #[default]
    Names,
    /// Its input and output too, with secrets redacted, media left out, and each clipped to
    /// 64 KiB.
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

    /// Applies this policy to a tool call's input or output.
    pub(super) fn apply(self, payload: &mut Value) {
        match self {
            Self::Names => *payload = Value::Null,
            Self::Payloads => {
                let mut budget = TOOL_PAYLOAD_LIMIT;
                shape(payload, &mut budget);
                // Many values each too short to clip (a big JSON listing) can still add up past
                // the limit: a list keeps the items that fit (a harness reads it back as a list,
                // as Codex does a tool search's `tools`); anything else is kept as its clipped
                // text, in an object (which an input must stay) or a list as it was one.
                let fits = TOOL_PAYLOAD_LIMIT + TOOL_PAYLOAD_LIMIT / 4;
                if let Value::Array(items) = payload {
                    let mut size = 2;
                    let kept = items
                        .iter()
                        .take_while(|item| {
                            size += item.to_string().len() + 1;
                            size <= fits
                        })
                        .count();
                    items.truncate(kept.max(1));
                }
                if payload.to_string().len() > fits {
                    let text = Value::String(clip(&payload.to_string(), TOOL_PAYLOAD_LIMIT));
                    *payload = match payload {
                        Value::Object(_) => {
                            Value::Object(Map::from_iter([("clipped".into(), text)]))
                        }
                        Value::Array(_) => Value::Array(vec![text]),
                        _ => text,
                    };
                }
            }
        }
    }
}

/// `value` with every string redacted and, once `budget` bytes of them are kept, clipped (but
/// never one [`SHORT`] enough to be a name); media blocks become a note. Its shape stays as it
/// was, so a harness reads it back as the same kind of input or output. A string is text, kept
/// as written (a Codex function call's arguments are JSON text, redacted as text).
fn shape(value: &mut Value, budget: &mut usize) {
    match value {
        Value::Object(object) => match media_note(object) {
            Some(note) => *value = note,
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
                            redact_named(&name, text);
                        }
                    }
                }
                for (key, value) in object.iter_mut() {
                    *budget = budget.saturating_sub(key.len());
                    if let Value::String(text) = value {
                        redact_named(key, text);
                    }
                    shape(value, budget);
                }
            }
        },
        Value::Array(items) => items.iter_mut().for_each(|v| shape(v, budget)),
        Value::String(text) => {
            let redacted = atuin_common::secrets::redact(text).into_owned();
            *text = clip(&redacted, (*budget).max(SHORT));
            *budget = budget.saturating_sub(redacted.len());
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

/// `text`, the value of `key`, redacted as the assignment it is: a secret is often known by the
/// name it is kept under (`{"AWS_SECRET_ACCESS_KEY": "..."}`), which the value alone doesn't say.
fn redact_named(key: &str, text: &mut String) {
    let line = format!("{key}={text}");
    let redacted = atuin_common::secrets::redact(&line);
    if redacted != line {
        let value = redacted.strip_prefix(key).and_then(|rest| rest.strip_prefix('='));
        value.unwrap_or("****").clone_into(text);
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

/// `text` cut to about `keep` bytes, its start and end kept around a note of how much was left
/// out.
fn clip(text: &str, keep: usize) -> String {
    if text.len() <= keep {
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
        ToolCapture::Payloads.apply(&mut value);
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
    fn secrets_are_redacted(#[case] value: Value) {
        let kept = kept(value).to_string();
        assert!(!kept.contains("PRIVATEKEY"), "{kept}");
        assert!(kept.contains("AWS_SECRET_ACCESS_KEY"), "the name stays: {kept}");
    }

    #[rstest]
    fn names_only_keeps_nothing() {
        let mut value = json!({"command": "ls"});
        ToolCapture::Names.apply(&mut value);
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

    /// JSON text (a Codex function call's arguments, a `cat` of a JSON file) is text: kept as
    /// written, not re-formatted.
    #[rstest]
    fn json_text_is_kept_as_written() {
        let text = "{\n  \"name\": \"x\",\n  \"n\": 12345678901234567890\n}";
        assert_eq!(kept(json!(text)), json!(text));
    }

    /// A payload of many values each too short to clip is still held to the limit.
    #[rstest]
    #[case::listing(Value::Array((0..20_000).map(|n| json!({"name": format!("pkg-{n}"), "version": "1.0.0"})).collect()))]
    #[case::input(json!({"files": (0..20_000).map(|n| format!("src/{n}.rs")).collect::<Vec<_>>()}))]
    fn many_short_values_are_held_to_the_limit(#[case] value: Value) {
        let (object, array) = (value.is_object(), value.is_array());
        let kept = kept(value);
        assert_eq!(kept.is_array(), array, "a list stays a list");
        // Kept as its text, which escaped (as stored) is a little longer.
        let size = kept.to_string().len();
        assert!(size < TOOL_PAYLOAD_LIMIT + TOOL_PAYLOAD_LIMIT / 4, "{size}");
        assert_eq!(kept.is_object(), object, "an input stays an object");
    }

    proptest! {
        /// A payload is held to the limit (its text escaped, as stored, a little over), whatever
        /// its strings hold, and a list stays a list.
        #[test]
        fn clipping_is_bounded_and_a_list_stays_one(
            strings in proptest::collection::vec(".{0,40000}", 0..4),
        ) {
            let value = Value::Array(strings.iter().cloned().map(Value::String).collect());
            let kept = kept(value);
            prop_assert!(kept.to_string().len() <= 2 * TOOL_PAYLOAD_LIMIT);
            // A list stays one, of its first items (all of them when they fit).
            let items = kept.as_array().unwrap();
            prop_assert!(items.len() <= strings.len() && (strings.is_empty() || !items.is_empty()));
        }

        #[test]
        fn clip_keeps_text_that_fits_whole(text in ".{0,50}", keep in 200usize..400) {
            prop_assert_eq!(clip(&text, keep), text);
        }
    }
}
