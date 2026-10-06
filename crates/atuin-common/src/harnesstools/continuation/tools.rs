//! Tool calls carried from one harness's session into another's, as calls of the target: each
//! translated to the target's own tool where it has one that does the same (a shell command, a
//! file read, written or edited, a search), else kept as the tool it was, under a name and id the
//! target's API takes. A model reads a call to a tool it doesn't have, and its result, like any
//! other: it just can't make that call again.

use std::collections::HashSet;
use std::path::Path;

use serde_json::{Map, Value, json};

use crate::harnesstools::note::command_line;
use crate::harnesstools::rehydrate::UNCAPTURED_OUTPUT;
use crate::harnesstools::session::{ToolCallId, ToolResult, ToolUse};
use crate::harnesstools::{AnyHarness, Harness as _};

/// The longest tool name or call id the model APIs take (Anthropic's and OpenAI's tool names
/// are `^[a-zA-Z0-9_-]{1,64}$`).
const MAX_NAME: usize = 64;

/// What a call did, in terms more than one harness has a tool for.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Canonical {
    Shell {
        command: String,
    },
    Read {
        path: String,
        offset: Option<Value>,
        limit: Option<Value>,
    },
    Write {
        path: String,
        content: String,
    },
    Edit {
        path: String,
        old: String,
        new: String,
        all: bool,
    },
    Grep {
        pattern: String,
        path: Option<String>,
        glob: Option<String>,
    },
    Glob {
        pattern: String,
        path: Option<String>,
    },
}

/// A call's input as an object: Codex keeps a function call's arguments as the JSON text the
/// model wrote.
pub(super) fn object(input: &Value) -> Option<Map<String, Value>> {
    match input {
        Value::Object(map) => Some(map.clone()),
        Value::String(text) => serde_json::from_str(text).ok(),
        _ => None,
    }
}

fn string(input: &Map<String, Value>, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|k| input.get(*k)?.as_str()).map(str::to_owned)
}

/// A command given as a line or as its arguments.
fn command(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(line) if !line.trim().is_empty() => Some(line.clone()),
        Value::Array(argv) if !argv.is_empty() => Some(command_line(argv)),
        _ => None,
    }
}

impl Canonical {
    /// What the call `name` with `input` did, when it is one of these; a command run elsewhere
    /// than `ran_in`, the directory the session ran in (Codex's `workdir`), runs there first:
    /// within it, as the same place under the directory the continuation resumes in.
    fn parse(name: &str, input: &Value, ran_in: &Path) -> Option<Self> {
        let kind = name.to_ascii_lowercase();
        // Before the input is looked at: most calls are of other tools.
        let known = matches!(
            kind.as_str(),
            "bash"
                | "shell"
                | "shell_command"
                | "exec_command"
                | "local_shell"
                | "read"
                | "write"
                | "edit"
                | "grep"
                | "glob"
                | "find"
        );
        if !known {
            return None;
        }
        let input = object(input)?;
        let path = || string(&input, &["file_path", "filePath", "path"]);
        Some(match kind.as_str() {
            "bash" | "shell" | "shell_command" | "exec_command" | "local_shell" => {
                let (command, workdir) = if name == "local_shell" {
                    (command(input.get("command")), string(&input, &["working_directory"]))
                } else {
                    (
                        command(input.get("command").or_else(|| input.get("cmd"))),
                        string(&input, &["workdir"]),
                    )
                };
                let mut command = command?;
                let dir = workdir.and_then(|dir| match Path::new(&dir).strip_prefix(ran_in) {
                    Ok(within) if within.as_os_str().is_empty() => None,
                    Ok(within) => Some(within.to_string_lossy().into_owned()),
                    Err(_) => Some(dir),
                });
                if let Some(dir) = dir {
                    let dir = shlex::try_quote(&dir).ok()?;
                    command = format!("cd {dir} && {command}");
                }
                Self::Shell { command }
            }
            "read" => Self::Read {
                path: path()?,
                offset: input.get("offset").filter(|v| v.is_number()).cloned(),
                limit: input.get("limit").filter(|v| v.is_number()).cloned(),
            },
            "write" => Self::Write {
                path: path()?,
                content: string(&input, &["content"])?,
            },
            "edit" => {
                // pi's edit takes its replacements as `edits`; only one is an edit the others make.
                let one = match input.get("edits") {
                    Some(Value::Array(edits)) => match edits.as_slice() {
                        [Value::Object(edit)] => edit.clone(),
                        _ => return None,
                    },
                    _ => input.clone(),
                };
                Self::Edit {
                    path: path()?,
                    old: string(&one, &["old_string", "oldString", "oldText"])?,
                    new: string(&one, &["new_string", "newString", "newText"])?,
                    all: ["replace_all", "replaceAll"]
                        .iter()
                        .any(|k| one.get(*k).and_then(Value::as_bool) == Some(true)),
                }
            }
            "grep" => Self::Grep {
                pattern: string(&input, &["pattern"])?,
                path: string(&input, &["path"]),
                glob: string(&input, &["glob", "include"]),
            },
            // pi's `find` lists files by a glob.
            "glob" | "find" => Self::Glob {
                pattern: string(&input, &["pattern"])?,
                path: string(&input, &["path"]),
            },
            _ => return None,
        })
    }

    /// This call as `target`'s own tool, when it has one that does the same: its name and input.
    fn render(&self, target: AnyHarness, cwd: &Path) -> Option<(&'static str, Value)> {
        use AnyHarness::{ClaudeCode, Codex, Opencode, Pi};

        let mut input = Map::new();
        let mut put = |key: &str, value: Option<Value>| {
            if let Some(value) = value {
                input.insert(key.to_owned(), value);
            }
        };
        let name = match (self, target) {
            (Self::Shell { command }, Codex(_)) => {
                put("cmd", Some(json!(command)));
                put("workdir", Some(json!(cwd)));
                "exec_command"
            }
            // Codex runs everything else through its shell.
            (_, Codex(_)) => return None,
            (Self::Shell { command }, _) => {
                put("command", Some(json!(command)));
                match target {
                    ClaudeCode(_) => "Bash",
                    _ => "bash",
                }
            }
            (
                Self::Read {
                    path,
                    offset,
                    limit,
                },
                _,
            ) => {
                put(path_key(target), Some(json!(path)));
                put("offset", offset.clone());
                put("limit", limit.clone());
                match target {
                    ClaudeCode(_) => "Read",
                    _ => "read",
                }
            }
            (Self::Write { path, content }, _) => {
                put(path_key(target), Some(json!(path)));
                put("content", Some(json!(content)));
                match target {
                    ClaudeCode(_) => "Write",
                    _ => "write",
                }
            }
            // pi's edit replaces one occurrence only.
            (Self::Edit { all: true, .. }, Pi(_)) => return None,
            (
                Self::Edit {
                    path,
                    old,
                    new,
                    all,
                },
                _,
            ) => {
                put(path_key(target), Some(json!(path)));
                let (old_key, new_key, all_key) = match target {
                    ClaudeCode(_) => ("old_string", "new_string", "replace_all"),
                    Opencode(_) => ("oldString", "newString", "replaceAll"),
                    _ => ("oldText", "newText", ""),
                };
                put(old_key, Some(json!(old)));
                put(new_key, Some(json!(new)));
                put(all_key, all.then_some(json!(true)));
                match target {
                    ClaudeCode(_) => "Edit",
                    _ => "edit",
                }
            }
            (
                Self::Grep {
                    pattern,
                    path,
                    glob,
                },
                _,
            ) => {
                put("pattern", Some(json!(pattern)));
                put("path", path.as_ref().map(|p| json!(p)));
                let glob_key = match target {
                    Opencode(_) => "include",
                    _ => "glob",
                };
                put(glob_key, glob.as_ref().map(|g| json!(g)));
                match target {
                    ClaudeCode(_) => "Grep",
                    _ => "grep",
                }
            }
            (Self::Glob { pattern, path }, _) => {
                put("pattern", Some(json!(pattern)));
                put("path", path.as_ref().map(|p| json!(p)));
                match target {
                    ClaudeCode(_) => "Glob",
                    Pi(_) => "find",
                    _ => "glob",
                }
            }
        };
        Some((name, Value::Object(input)))
    }
}

/// The key a file tool of `target` names its file by.
const fn path_key(target: AnyHarness) -> &'static str {
    match target {
        AnyHarness::ClaudeCode(_) => "file_path",
        AnyHarness::Opencode(_) => "filePath",
        AnyHarness::Codex(_) | AnyHarness::Pi(_) => "path",
    }
}

/// `text` as a tool name or call id the APIs take: what they don't allow becomes `_`.
fn api_name(text: &str, empty: &str) -> String {
    let name: String = text
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .take(MAX_NAME)
        .collect();
    if name.is_empty() {
        empty.to_owned()
    } else {
        name
    }
}

/// A call's input as `target` writes one: Codex a function call's arguments as JSON text (and a
/// custom tool's freeform input as it is), the others an object.
fn input_for(target: AnyHarness, input: Value) -> Value {
    match (target, input) {
        (AnyHarness::Codex(_), Value::Object(map)) => Value::String(Value::Object(map).to_string()),
        (AnyHarness::Codex(_), input @ Value::String(_)) => input,
        (AnyHarness::Codex(_), input) => Value::String(input.to_string()),
        (_, Value::Object(map)) => Value::Object(map),
        (_, input) => match object(&input) {
            Some(map) => Value::Object(map),
            None => json!({ "input": input }),
        },
    }
}

/// A result's output as text, which every target writes as a tool's output: the text of its
/// blocks (Claude Code's and pi's `text`, Codex's `input_text`), else its JSON.
pub(super) fn output_text(output: &Value) -> String {
    match output {
        Value::Null => UNCAPTURED_OUTPUT.to_owned(),
        Value::String(text) => text.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .map(|block| match block {
                Value::String(text) => text.clone(),
                _ => block["text"].as_str().map_or_else(|| block.to_string(), str::to_owned),
            })
            .collect::<Vec<_>>()
            .join("\n"),
        other => other.to_string(),
    }
}

/// Call ids for the target, unique within the session.
#[derive(Debug, Default)]
pub(super) struct CallIds(HashSet<String>);

impl CallIds {
    fn mint(&mut self, id: &ToolCallId) -> ToolCallId {
        let base = api_name(id.as_ref(), "call");
        let mut id = base.clone();
        let mut n = 1;
        while !self.0.insert(id.clone()) {
            n += 1;
            let suffix = format!("_{n}");
            id = format!("{}{suffix}", &base[..base.len().min(MAX_NAME - suffix.len())]);
        }
        ToolCallId::from(id)
    }
}

/// Whether `name` is one of `target`'s own tools, or a name its writer reads as one of its own
/// items (Codex's): a call kept as the tool it was, under such a name, would read as that tool.
fn native(target: AnyHarness, name: &str) -> bool {
    let names: &[&str] = match target {
        AnyHarness::ClaudeCode(_) => &[
            "bash",
            "read",
            "write",
            "edit",
            "multiedit",
            "notebookedit",
            "grep",
            "glob",
            "task",
            "agent",
            "webfetch",
            "websearch",
            "todowrite",
        ],
        AnyHarness::Codex(_) => &[
            "exec_command",
            "shell",
            "shell_command",
            "local_shell",
            "apply_patch",
            "write_stdin",
            "update_plan",
            "view_image",
            "web_search",
            "image_generation",
            "tool_search",
        ],
        AnyHarness::Opencode(_) => &[
            "bash",
            "read",
            "write",
            "edit",
            "grep",
            "glob",
            "list",
            "task",
            "webfetch",
            "websearch",
            "todowrite",
            "todoread",
            "patch",
        ],
        AnyHarness::Pi(_) => &["bash", "read", "write", "edit", "grep", "find", "ls"],
    };
    names.contains(&name.to_ascii_lowercase().as_str())
}

/// `call` (made in `source`, in a session that ran in `ran_in`) and its `result` as `target`
/// writes them, in a session resuming in `cwd`. A call kept as the tool it was, whose name is one of the target's own tools (which
/// takes other input), is named for its source: `pi_edit`.
pub(super) fn carry(
    source: AnyHarness,
    target: AnyHarness,
    call: &ToolUse,
    result: &ToolResult,
    ran_in: &Path,
    cwd: &Path,
    ids: &mut CallIds,
) -> (ToolUse, ToolResult) {
    let (name, input) = Canonical::parse(&call.name, &call.input, ran_in)
        .and_then(|canonical| canonical.render(target, cwd))
        .map_or_else(
            || {
                let name = if native(target, &call.name) {
                    format!("{}_{}", source.name(), call.name)
                } else {
                    call.name.clone()
                };
                (api_name(&name, "tool"), call.input.clone())
            },
            |(name, input)| (name.to_owned(), input),
        );
    let id = ids.mint(&call.id);
    let call = ToolUse {
        id: id.clone(),
        name,
        input: input_for(target, input),
    };
    let result = ToolResult {
        call: id,
        output: Value::String(output_text(&result.output)),
        error: result.error,
    };
    (call, result)
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;
    use crate::harnesstools::continuation::tests::{CLAUDE, CODEX, OPENCODE, PI};

    fn carried(source_name: &str, input: Value, target: AnyHarness) -> (String, Value) {
        let call = ToolUse {
            id: ToolCallId::from("call_1".to_owned()),
            name: source_name.to_owned(),
            input,
        };
        let result = ToolResult {
            call: call.id.clone(),
            output: json!("ok"),
            error: false,
        };
        let source = if target.name() == CLAUDE.name() {
            PI
        } else {
            CLAUDE
        };
        // Resumed elsewhere than it ran (another machine).
        let (ran_in, cwd) = (Path::new("/work"), Path::new("/here"));
        let (call, _) = carry(source, target, &call, &result, ran_in, cwd, &mut CallIds::default());
        (call.name, call.input)
    }

    #[rstest]
    #[case::claude_to_opencode("Bash", json!({"command": "ls", "description": "list"}), OPENCODE, "bash", json!({"command": "ls"}))]
    #[case::codex_to_claude("exec_command", json!(r#"{"cmd":"cargo test","workdir":"/work"}"#), CLAUDE, "Bash", json!({"command": "cargo test"}))]
    #[case::codex_within("exec_command", json!(r#"{"cmd":"cargo test","workdir":"/work/crates/x"}"#), CLAUDE, "Bash", json!({"command": "cd crates/x && cargo test"}))]
    #[case::codex_elsewhere("exec_command", json!(r#"{"cmd":"ls","workdir":"/tmp/x y"}"#), PI, "bash", json!({"command": "cd '/tmp/x y' && ls"}))]
    #[case::codex_argv("shell", json!({"command": ["bash", "-lc", "rg foo"]}), CLAUDE, "Bash", json!({"command": "rg foo"}))]
    #[case::claude_to_codex("Bash", json!({"command": "ls"}), CODEX, "exec_command", json!(r#"{"cmd":"ls","workdir":"/here"}"#))]
    #[case::read("Read", json!({"file_path": "/a.rs", "limit": 20}), OPENCODE, "read", json!({"filePath": "/a.rs", "limit": 20}))]
    #[case::edit_to_pi("edit", json!({"filePath": "/a.rs", "oldString": "a", "newString": "b"}), PI, "edit", json!({"path": "/a.rs", "oldText": "a", "newText": "b"}))]
    #[case::pi_edit("edit", json!({"path": "/a.rs", "edits": [{"oldText": "a", "newText": "b"}]}), CLAUDE, "Edit", json!({"file_path": "/a.rs", "old_string": "a", "new_string": "b"}))]
    #[case::replace_all_kept_foreign("Edit", json!({"file_path": "/a", "old_string": "a", "new_string": "b", "replace_all": true}), PI, "claude-code_Edit", json!({"file_path": "/a", "old_string": "a", "new_string": "b", "replace_all": true}))]
    #[case::multi_edit_kept_foreign("edit", json!({"path": "/a", "edits": [{"oldText": "a", "newText": "b"}, {"oldText": "c", "newText": "d"}]}), CLAUDE, "pi_edit", json!({"path": "/a", "edits": [{"oldText": "a", "newText": "b"}, {"oldText": "c", "newText": "d"}]}))]
    #[case::codex_item_name("web_search", json!({"query": "atuin"}), CODEX, "claude-code_web_search", json!(r#"{"query":"atuin"}"#))]
    #[case::glob_to_pi("Glob", json!({"pattern": "**/*.rs"}), PI, "find", json!({"pattern": "**/*.rs"}))]
    #[case::grep("grep", json!({"pattern": "fn main", "include": "*.rs"}), CLAUDE, "Grep", json!({"pattern": "fn main", "glob": "*.rs"}))]
    #[case::read_to_codex("Read", json!({"file_path": "/a.rs"}), CODEX, "Read", json!(r#"{"file_path":"/a.rs"}"#))]
    #[case::mcp("mcp__atuin__ai_session_read", json!({"id": "x"}), OPENCODE, "mcp__atuin__ai_session_read", json!({"id": "x"}))]
    #[case::patch_freeform("apply_patch", json!("*** Begin Patch\n*** End Patch"), CLAUDE, "apply_patch", json!({"input": "*** Begin Patch\n*** End Patch"}))]
    #[case::odd_name("atuin.history:search", json!({}), CLAUDE, "atuin_history_search", json!({}))]
    fn calls_translate(
        #[case] name: &str,
        #[case] input: Value,
        #[case] target: AnyHarness,
        #[case] want_name: &str,
        #[case] want_input: Value,
    ) {
        assert_eq!(carried(name, input, target), (want_name.to_owned(), want_input));
    }

    #[rstest]
    #[case::string(json!("3 passed"), "3 passed")]
    #[case::blocks(json!([{"type": "text", "text": "a"}, {"type": "input_text", "text": "b"}]), "a\nb")]
    #[case::uncaptured(Value::Null, UNCAPTURED_OUTPUT)]
    #[case::structured(json!({"exit": 0}), r#"{"exit":0}"#)]
    fn outputs_become_text(#[case] output: Value, #[case] want: &str) {
        assert_eq!(output_text(&output), want);
    }

    #[rstest]
    fn call_ids_are_ones_the_apis_take_and_unique() {
        let mut ids = CallIds::default();
        let a = ids.mint(&ToolCallId::from("call_x|fc_y".to_owned()));
        let b = ids.mint(&ToolCallId::from("call_x_fc_y".to_owned()));
        assert_eq!(a.as_ref(), "call_x_fc_y");
        assert_eq!(b.as_ref(), "call_x_fc_y_2");
        let long = ids.mint(&ToolCallId::from("x".repeat(100)));
        assert_eq!(long.as_ref().len(), MAX_NAME);
    }
}
