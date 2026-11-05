//! Tool calls carried from one harness's session into another's, as calls of the target: each
//! translated to the target's own tool where it has one that does the same (a shell command, a
//! file read, written or edited, a search), else kept as the tool it was, under a name and id the
//! target's API takes. A model reads a call to a tool it doesn't have, and its result, like any
//! other: it just can't make that call again.

use std::borrow::Cow;
use std::collections::HashSet;
use std::path::Path;

use serde_json::{Map, Value, json};

use crate::harnesstools::access::script::{self, Call};
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
        /// Matches letters of either case (Claude Code's `-i`, pi's `ignoreCase`).
        insensitive: bool,
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
    /// What the call `name` with `input`, made in `source`, did, when it is one of these; a
    /// command run elsewhere than `ran_in`, the directory the session ran in (Codex's `workdir`),
    /// runs there first: within it, as the same place under the directory the continuation
    /// resumes in.
    fn parse(source: AnyHarness, name: &str, input: &Value, ran_in: &Path) -> Option<Self> {
        let kind = name.to_ascii_lowercase();
        // Before the input is looked at: most calls are of other tools. Kiro's `fs_append` (no
        // other harness appends), `file_search` (two schemas in its bundle: a substring, a glob)
        // and `list_directory` stay the tools they were.
        let known = matches!(
            kind.as_str(),
            "bash"
                | "shell"
                | "shell_command"
                | "exec_command"
                | "local_shell"
                | "run_terminal_command"
                | "execute_bash"
                | "execute_pwsh"
                | "read"
                | "read_file"
                | "view"
                | "write"
                | "create"
                | "fs_write"
                | "edit"
                | "search_replace"
                | "str_replace"
                | "grep"
                | "grep_search"
                | "glob"
                | "find"
        );
        if !known {
            return None;
        }
        let input = object(input)?;
        let path = || string(&input, &["file_path", "filePath", "target_file", "path"]);
        Some(match kind.as_str() {
            "bash"
            | "shell"
            | "shell_command"
            | "exec_command"
            | "local_shell"
            | "run_terminal_command"
            | "execute_bash"
            | "execute_pwsh" => {
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
            "read" | "read_file" => Self::Read {
                path: path()?,
                offset: match source {
                    // Kiro's `read_file` counts lines from 0, the others from 1.
                    AnyHarness::Kiro(_) => {
                        input.get("offset").and_then(Value::as_u64).map(|n| json!(n + 1))
                    }
                    _ => input.get("offset").filter(|v| v.is_number()).cloned(),
                },
                limit: input.get("limit").filter(|v| v.is_number()).cloned(),
            },
            // Copilot's `view` reads a range of lines, `[first, last]`, 1-based.
            "view" => {
                let range = input.get("view_range").and_then(Value::as_array);
                let line = |n: usize| range.and_then(|r| r.get(n)?.as_u64());
                let (first, last) = (line(0), line(1));
                Self::Read {
                    path: path()?,
                    offset: first.map(|n| json!(n)),
                    limit: first
                        .zip(last)
                        .map(|(first, last)| json!(last.saturating_sub(first) + 1)),
                }
            }
            "write" | "create" | "fs_write" => Self::Write {
                path: path()?,
                content: string(&input, &["content", "file_text", "text"])?,
            },
            "edit" | "search_replace" | "str_replace" => {
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
                    old: string(&one, &[
                        "old_string",
                        "oldString",
                        "oldText",
                        "old_str",
                        "oldStr",
                    ])?,
                    new: string(&one, &[
                        "new_string",
                        "newString",
                        "newText",
                        "new_str",
                        "newStr",
                    ])?,
                    all: ["replace_all", "replaceAll"]
                        .iter()
                        .any(|k| one.get(*k).and_then(Value::as_bool) == Some(true)),
                }
            }
            "grep" => Self::Grep {
                pattern: string(&input, &["pattern"])?,
                path: string(&input, &["path"]),
                glob: string(&input, &["glob", "include"]),
                insensitive: ["-i", "ignoreCase"]
                    .iter()
                    .any(|k| input.get(*k).and_then(Value::as_bool) == Some(true)),
            },
            // Kiro's search covers the whole workspace, and ignores case unless told otherwise.
            // The others have no way to leave files out.
            "grep_search" => {
                if input.get("excludePattern").is_some_and(|v| !v.is_null()) {
                    return None;
                }
                Self::Grep {
                    pattern: string(&input, &["query"])?,
                    path: None,
                    glob: string(&input, &["includePattern"]),
                    insensitive: input.get("caseSensitive").and_then(Value::as_bool) != Some(true),
                }
            }
            // pi's `find` lists files by a glob.
            "glob" | "find" => Self::Glob {
                pattern: string(&input, &["pattern"])?,
                path: string(&input, &["path"]),
            },
            _ => return None,
        })
    }

    /// What the calls of a Codex `exec` script did, when they are all shell commands: one
    /// command running each in turn, in a shell of its own as each call had.
    fn shells(source: AnyHarness, calls: &[Call], ran_in: &Path) -> Option<Self> {
        let commands = calls
            .iter()
            .map(|call| match Self::parse(source, &call.name, &call.input, ran_in)? {
                Self::Shell { command } => Some(format!("({command}\n)")),
                _ => None,
            })
            .collect::<Option<Vec<_>>>()?;
        (commands.len() > 1).then(|| Self::Shell {
            command: commands.join("\n"),
        })
    }

    /// This call as `target`'s own tool, when it has one that does the same: its name and input.
    fn render(&self, target: AnyHarness, cwd: &Path) -> Option<(&'static str, Value)> {
        use AnyHarness::{ClaudeCode, Codex, Copilot, Grok, Kiro, Opencode, Pi};

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
                    Grok(_) => "run_terminal_command",
                    Kiro(_) => "execute_bash",
                    _ => "bash",
                }
            }
            // Copilot's `view` reads a range of lines, `[first, last]`, 1-based: a read of
            // `limit` lines from `offset`, or the whole file. A read bounded at one end only
            // has no faithful range, so it stays the tool it was.
            (
                Self::Read {
                    path,
                    offset,
                    limit,
                },
                Copilot(_),
            ) => {
                put("path", Some(json!(path)));
                let first = offset.as_ref().and_then(Value::as_u64);
                let count = limit.as_ref().and_then(Value::as_u64);
                match (first, count) {
                    (Some(first), Some(count)) => {
                        put("view_range", Some(json!([first, first + count.saturating_sub(1)])));
                    }
                    (None, None) => {}
                    _ => return None,
                }
                "view"
            }
            (
                Self::Read {
                    path,
                    offset,
                    limit,
                },
                _,
            ) => {
                let key = match target {
                    Grok(_) => "target_file",
                    _ => path_key(target),
                };
                put(key, Some(json!(path)));
                let offset = match target {
                    // Kiro counts lines from 0 (see `parse`).
                    Kiro(_) => {
                        offset.as_ref().and_then(Value::as_u64).map(|n| json!(n.saturating_sub(1)))
                    }
                    _ => offset.clone(),
                };
                put("offset", offset);
                put("limit", limit.clone());
                match target {
                    ClaudeCode(_) => "Read",
                    Grok(_) => "read_file",
                    Kiro(_) => "read_file",
                    _ => "read",
                }
            }
            (Self::Write { path, content }, Kiro(_)) => {
                put("path", Some(json!(path)));
                put("text", Some(json!(content)));
                "fs_write"
            }
            (Self::Write { path, content }, Copilot(_)) => {
                put("path", Some(json!(path)));
                put("file_text", Some(json!(content)));
                "create"
            }
            (Self::Write { path, content }, _) => {
                put(path_key(target), Some(json!(path)));
                put("content", Some(json!(content)));
                match target {
                    ClaudeCode(_) => "Write",
                    _ => "write",
                }
            }
            // pi's and Copilot's edits replace one occurrence only.
            (Self::Edit { all: true, .. }, Pi(_) | Copilot(_)) => return None,
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
                    ClaudeCode(_) | Grok(_) => ("old_string", "new_string", "replace_all"),
                    Opencode(_) => ("oldString", "newString", "replaceAll"),
                    Kiro(_) => ("oldStr", "newStr", "replace_all"),
                    Copilot(_) => ("old_str", "new_str", ""),
                    _ => ("oldText", "newText", ""),
                };
                put(old_key, Some(json!(old)));
                put(new_key, Some(json!(new)));
                put(all_key, all.then_some(json!(true)));
                match target {
                    ClaudeCode(_) => "Edit",
                    Grok(_) => "search_replace",
                    Kiro(_) => "str_replace",
                    _ => "edit",
                }
            }
            // Copilot's searches (`rg`, `glob`) take input atuin hasn't seen: a search stays
            // the tool it was, which the model reads like any other. So does a `Glob` into Kiro,
            // whose `file_search` may take a substring rather than a glob (see `parse`).
            (Self::Grep { .. } | Self::Glob { .. }, Copilot(_)) | (Self::Glob { .. }, Kiro(_)) => {
                return None;
            }
            // Kiro's `grep_search` searches the whole workspace: one under a path stays foreign.
            (Self::Grep { path: Some(_), .. }, Kiro(_)) => return None,
            (
                Self::Grep {
                    pattern,
                    glob,
                    insensitive,
                    ..
                },
                Kiro(_),
            ) => {
                put("query", Some(json!(pattern)));
                // Kiro's `includePattern` needs `**` to reach into directories; a glob with no `/`
                // names files in any of them, as Claude Code's does.
                let glob = glob.as_ref().map(|g| {
                    if g.contains('/') {
                        g.clone()
                    } else {
                        format!("**/{g}")
                    }
                });
                put("includePattern", glob.map(|g| json!(g)));
                // Kiro ignores case unless told otherwise.
                put("caseSensitive", (!insensitive).then_some(json!(true)));
                "grep_search"
            }
            // opencode's search can't ignore case.
            (
                Self::Grep {
                    insensitive: true, ..
                },
                Opencode(_),
            ) => return None,
            (
                Self::Grep {
                    pattern,
                    path,
                    glob,
                    insensitive,
                },
                _,
            ) => {
                put("pattern", Some(json!(pattern)));
                let case_key = match target {
                    Pi(_) => "ignoreCase",
                    _ => "-i",
                };
                put(case_key, insensitive.then_some(json!(true)));
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
        AnyHarness::ClaudeCode(_) | AnyHarness::Grok(_) => "file_path",
        AnyHarness::Opencode(_) => "filePath",
        AnyHarness::Amp(_)
        | AnyHarness::Codex(_)
        | AnyHarness::Copilot(_)
        | AnyHarness::Kiro(_)
        | AnyHarness::Pi(_) => "path",
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

/// A result's output as text, which every target writes as a tool's output (see
/// [`ToolResult::output_text`]).
pub(super) fn output_text(result: &ToolResult) -> String {
    result.output_text().map_or_else(|| UNCAPTURED_OUTPUT.to_owned(), Cow::into_owned)
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
        AnyHarness::Amp(_) | AnyHarness::ClaudeCode(_) => &[
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
        AnyHarness::Grok(_) => &[
            "run_terminal_command",
            "read_file",
            "write",
            "search_replace",
            "hashline_edit",
            "delete_file",
            "grep",
            "glob",
            "list_dir",
            "spawn_subagent",
            "todo_write",
            "web_search",
            "web_fetch",
            "lsp",
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
        // What Copilot CLI 1.0.104 offers the model, its agent-management tools aside.
        AnyHarness::Copilot(_) => &[
            "bash",
            "read_bash",
            "write_bash",
            "stop_bash",
            "list_bash",
            "view",
            "edit",
            "create",
            "apply_patch",
            "glob",
            "grep",
            "rg",
            "task",
            "web_fetch",
            "ask_user",
            "skill",
            "sql",
            "store_memory",
            "vote_memory",
        ],
        // Kiro CLI 2.36.0's built-in tool ids (`execute_pwsh` is the shell on Windows).
        AnyHarness::Kiro(_) => &[
            "execute_bash",
            "execute_pwsh",
            "read_file",
            "list_directory",
            "file_search",
            "grep_search",
            "fs_write",
            "fs_append",
            "str_replace",
            "delete_file",
            "web_fetch",
            "remote_web_search",
            "invoke_sub_agent",
            "code",
            "introspect",
            "knowledge",
            "disclose_context",
        ],
    };
    names.contains(&name.to_ascii_lowercase().as_str())
}

/// The tool calls a Codex `exec` call's script makes (see the `script` module), when it was
/// read whole and each ran once, in turn.
fn script_calls(source: AnyHarness, call: &ToolUse) -> Option<Vec<Call>> {
    let script = match (source, &call.input) {
        (AnyHarness::Codex(_), Value::String(text)) if call.name == "exec" => script::calls(text),
        _ => return None,
    };
    (script.whole && script.straight).then_some(script.calls)
}

/// `call` (made in `source`, in a session that ran in `ran_in`) and its `result` as `target`
/// writes them, in a session resuming in `cwd`. A call kept as the tool it was, whose name is one of the target's own tools (which
/// takes other input), is named for its source: `pi_edit`. A Codex `exec` script making one call
/// is carried as that call; one making several, as one shell command when they all are.
pub(super) fn carry(
    source: AnyHarness,
    target: AnyHarness,
    call: &ToolUse,
    result: &ToolResult,
    ran_in: &Path,
    cwd: &Path,
    ids: &mut CallIds,
) -> (ToolUse, ToolResult) {
    let calls = script_calls(source, call);
    let (name, input) = match calls.as_deref() {
        Some([one]) => (&one.name, &one.input),
        _ => (&call.name, &call.input),
    };
    let (name, input) = Canonical::parse(source, name, input, ran_in)
        .or_else(|| Canonical::shells(source, calls.as_deref()?, ran_in))
        .and_then(|canonical| canonical.render(target, cwd))
        .map_or_else(
            || {
                let name = if native(target, name) {
                    format!("{}_{}", source.name(), name)
                } else {
                    name.clone()
                };
                (api_name(&name, "tool"), input.clone())
            },
            |(name, input)| (name.to_owned(), input),
        );
    let id = ids.mint(&call.id);
    // Kiro's shell tool reports its output with the command's exit code, which capture didn't
    // keep: a command that failed is given 1, so the model still reads it as failed.
    let output = if matches!(target, AnyHarness::Kiro(_)) && name == "execute_bash" {
        let code = if result.error {
            1
        } else {
            0
        };
        format!("Output:\n{}\n\n\nExit Code: {code}", output_text(result))
    } else {
        output_text(result)
    };
    let call = ToolUse {
        id: id.clone(),
        name,
        input: input_for(target, input),
    };
    let result = ToolResult {
        call: id,
        output: Value::String(output),
        error: result.error,
    };
    (call, result)
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;
    use crate::harnesstools::continuation::tests::{
        CLAUDE, CODEX, COPILOT, GROK, KIRO, OPENCODE, PI,
    };

    /// Copilot as a target, for a case whose source must stay Claude Code (its name shows in
    /// a call kept foreign).
    const CLAUDE_TO_COPILOT: AnyHarness = COPILOT;

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
    #[case::claude_to_copilot("Bash", json!({"command": "ls"}), COPILOT, "bash", json!({"command": "ls"}))]
    #[case::read_to_copilot("Read", json!({"file_path": "/a.rs", "offset": 10, "limit": 5}), COPILOT, "view", json!({"path": "/a.rs", "view_range": [10, 14]}))]
    #[case::whole_read_to_copilot("Read", json!({"file_path": "/a.rs"}), COPILOT, "view", json!({"path": "/a.rs"}))]
    #[case::read_from_an_offset_kept_foreign_by_copilot("Read", json!({"file_path": "/a.rs", "offset": 10}), COPILOT, "Read", json!({"file_path": "/a.rs", "offset": 10}))]
    #[case::read_of_a_limit_kept_foreign_by_copilot("Read", json!({"file_path": "/a.rs", "limit": 5}), COPILOT, "Read", json!({"file_path": "/a.rs", "limit": 5}))]
    #[case::whole_read_to_copilot("Read", json!({"file_path": "/a.rs"}), COPILOT, "view", json!({"path": "/a.rs"}))]
    #[case::copilot_view_to_claude("view", json!({"path": "/a.rs", "view_range": [10, 14]}), CLAUDE, "Read", json!({"file_path": "/a.rs", "offset": 10, "limit": 5}))]
    #[case::write_to_copilot("Write", json!({"file_path": "/a.rs", "content": "x"}), COPILOT, "create", json!({"path": "/a.rs", "file_text": "x"}))]
    #[case::copilot_create_to_pi("create", json!({"path": "/a.rs", "file_text": "x"}), PI, "write", json!({"path": "/a.rs", "content": "x"}))]
    #[case::edit_to_copilot("Edit", json!({"file_path": "/a.rs", "old_string": "a", "new_string": "b"}), COPILOT, "edit", json!({"path": "/a.rs", "old_str": "a", "new_str": "b"}))]
    #[case::copilot_edit_to_claude("edit", json!({"path": "/a.rs", "old_str": "a", "new_str": "b"}), CLAUDE, "Edit", json!({"file_path": "/a.rs", "old_string": "a", "new_string": "b"}))]
    #[case::replace_all_kept_foreign_by_copilot("Edit", json!({"file_path": "/a", "old_string": "a", "new_string": "b", "replace_all": true}), COPILOT, "claude-code_Edit", json!({"file_path": "/a", "old_string": "a", "new_string": "b", "replace_all": true}))]
    #[case::grep_kept_foreign_by_copilot("Grep", json!({"pattern": "fn main"}), CLAUDE_TO_COPILOT, "claude-code_Grep", json!({"pattern": "fn main"}))]
    #[case::claude_to_kiro("Bash", json!({"command": "ls", "description": "list"}), KIRO, "execute_bash", json!({"command": "ls"}))]
    #[case::read_to_kiro("Read", json!({"file_path": "/a.rs", "offset": 10, "limit": 5}), KIRO, "read_file", json!({"path": "/a.rs", "offset": 9, "limit": 5}))]
    #[case::read_from_line_0_to_kiro("Read", json!({"file_path": "/a.rs", "offset": 0}), KIRO, "read_file", json!({"path": "/a.rs", "offset": 0}))]
    #[case::write_to_kiro("create", json!({"path": "/a.rs", "file_text": "x"}), KIRO, "fs_write", json!({"path": "/a.rs", "text": "x"}))]
    #[case::kiro_write_to_claude("fs_write", json!({"path": "/a.rs", "text": "x"}), CLAUDE, "Write", json!({"file_path": "/a.rs", "content": "x"}))]
    #[case::edit_to_kiro("Edit", json!({"file_path": "/a", "old_string": "a", "new_string": "b"}), KIRO, "str_replace", json!({"path": "/a", "oldStr": "a", "newStr": "b"}))]
    #[case::replace_all_to_kiro("Edit", json!({"file_path": "/a", "old_string": "a", "new_string": "b", "replace_all": true}), KIRO, "str_replace", json!({"path": "/a", "oldStr": "a", "newStr": "b", "replace_all": true}))]
    #[case::kiro_edit_to_claude("str_replace", json!({"path": "/a", "oldStr": "a", "newStr": "b", "replace_all": true}), CLAUDE, "Edit", json!({"file_path": "/a", "old_string": "a", "new_string": "b", "replace_all": true}))]
    #[case::kiro_edit_to_pi("str_replace", json!({"path": "/a", "oldStr": "a", "newStr": "b", "replace_all": false}), PI, "edit", json!({"path": "/a", "oldText": "a", "newText": "b"}))]
    #[case::grep_to_kiro("Grep", json!({"pattern": "fn main", "glob": "*.rs"}), KIRO, "grep_search", json!({"query": "fn main", "includePattern": "**/*.rs", "caseSensitive": true}))]
    #[case::grep_under_a_dir_to_kiro("Grep", json!({"pattern": "fn main", "glob": "src/**/*.rs"}), KIRO, "grep_search", json!({"query": "fn main", "includePattern": "src/**/*.rs", "caseSensitive": true}))]
    #[case::insensitive_grep_to_kiro("Grep", json!({"pattern": "fn main", "-i": true}), KIRO, "grep_search", json!({"query": "fn main"}))]
    #[case::kiro_grep_to_claude("grep_search", json!({"query": "fn main", "includePattern": "**/*.rs", "explanation": "find it"}), CLAUDE, "Grep", json!({"pattern": "fn main", "glob": "**/*.rs", "-i": true}))]
    #[case::kiro_case_sensitive_grep_to_claude("grep_search", json!({"query": "fn main", "caseSensitive": true}), CLAUDE, "Grep", json!({"pattern": "fn main"}))]
    #[case::insensitive_grep_to_pi("Grep", json!({"pattern": "fn main", "-i": true}), PI, "grep", json!({"pattern": "fn main", "ignoreCase": true}))]
    #[case::pi_insensitive_grep_to_claude("grep", json!({"pattern": "fn main", "ignoreCase": true}), CLAUDE, "Grep", json!({"pattern": "fn main", "-i": true}))]
    #[case::insensitive_grep_kept_foreign_by_opencode("Grep", json!({"pattern": "fn main", "-i": true}), OPENCODE, "claude-code_Grep", json!({"pattern": "fn main", "-i": true}))]
    #[case::kiro_grep_kept_foreign_by_opencode("grep_search", json!({"query": "fn main"}), OPENCODE, "grep_search", json!({"query": "fn main"}))]
    #[case::grep_under_a_path_kept_foreign_by_kiro("Grep", json!({"pattern": "fn main", "path": "/a"}), KIRO, "Grep", json!({"pattern": "fn main", "path": "/a"}))]
    #[case::kiro_grep_excluding_kept_foreign("grep_search", json!({"query": "fn main", "caseSensitive": true, "excludePattern": "target/**"}), CLAUDE, "grep_search", json!({"query": "fn main", "caseSensitive": true, "excludePattern": "target/**"}))]
    #[case::glob_kept_foreign_by_kiro("Glob", json!({"pattern": "**/*.rs"}), KIRO, "Glob", json!({"pattern": "**/*.rs"}))]
    #[case::kiro_append_kept_foreign("fs_append", json!({"path": "/a", "text": "x"}), CLAUDE, "fs_append", json!({"path": "/a", "text": "x"}))]
    #[case::kiro_file_search_kept_foreign("file_search", json!({"query": "main"}), CLAUDE, "file_search", json!({"query": "main"}))]
    #[case::kiro_list_kept_foreign("list_directory", json!({"path": "/a"}), CLAUDE, "list_directory", json!({"path": "/a"}))]
    fn calls_translate(
        #[case] name: &str,
        #[case] input: Value,
        #[case] target: AnyHarness,
        #[case] want_name: &str,
        #[case] want_input: Value,
    ) {
        assert_eq!(carried(name, input, target), (want_name.to_owned(), want_input));
    }

    /// Kiro's `read_file` counts lines from 0, and Grok Build's, of the same name, from 1 as
    /// atuin's canonical read does.
    #[rstest]
    #[case::kiro(KIRO, json!({"path": "/a.rs", "offset": 0, "limit": 5}), json!({"file_path": "/a.rs", "offset": 1, "limit": 5}))]
    #[case::kiro_whole(KIRO, json!({"path": "/a.rs", "offset": null, "limit": null}), json!({"file_path": "/a.rs"}))]
    #[case::grok(GROK, json!({"target_file": "/a.rs", "offset": 10, "limit": 5}), json!({"file_path": "/a.rs", "offset": 10, "limit": 5}))]
    fn a_read_file_keeps_its_sources_line_numbers(
        #[case] source: AnyHarness,
        #[case] input: Value,
        #[case] want: Value,
    ) {
        let call = ToolUse {
            id: ToolCallId::from("call_1".to_owned()),
            name: "read_file".to_owned(),
            input,
        };
        let result = ToolResult {
            call: call.id.clone(),
            output: json!("ok"),
            error: false,
        };
        let here = Path::new("/work");
        let (call, _) = carry(source, CLAUDE, &call, &result, here, here, &mut CallIds::default());
        assert_eq!((call.name.as_str(), call.input), ("Read", want));
    }

    /// Codex 0.160+ makes its calls from a JavaScript `exec` script: one call is carried as that
    /// call, several shell commands as one, and anything else as the script it was.
    #[rstest]
    #[case::shell(r#"text(await tools.exec_command({cmd: "cargo test", workdir: "/work"}));"#, CLAUDE, "Bash", json!({"command": "cargo test"}))]
    #[case::shell_to_copilot(r#"await tools.exec_command({cmd: "ls", workdir: "/work/x"})"#, COPILOT, "bash", json!({"command": "cd x && ls"}))]
    #[case::patch(r#"await tools.apply_patch("*** Begin Patch\n*** End Patch")"#, CLAUDE, "apply_patch", json!({"input": "*** Begin Patch\n*** End Patch"}))]
    #[case::patch_to_copilot(r#"await tools.apply_patch("*** Begin Patch\n*** End Patch")"#, COPILOT, "codex_apply_patch", json!({"input": "*** Begin Patch\n*** End Patch"}))]
    #[case::shells(r#"text(await tools.exec_command({cmd: "echo a", workdir: "/work"})); text(await tools.exec_command({cmd: "ls", workdir: "/work/x"}));"#, PI, "bash", json!({"command": "(echo a\n)\n(cd x && ls\n)"}))]
    #[case::shells_ending_in_a_comment(r#"await tools.exec_command({cmd: "echo a # note"}); await tools.exec_command({cmd: "ls"})"#, PI, "bash", json!({"command": "(echo a # note\n)\n(ls\n)"}))]
    #[case::shell_and_patch(r#"await tools.exec_command({cmd: "ls"}); await tools.apply_patch("*** Begin Patch\n*** End Patch")"#, CLAUDE, "exec", json!({"input": r#"await tools.exec_command({cmd: "ls"}); await tools.apply_patch("*** Begin Patch\n*** End Patch")"#}))]
    #[case::branch(r#"if (false) { await tools.exec_command({cmd: "touch done"}) }"#, CLAUDE, "exec", json!({"input": r#"if (false) { await tools.exec_command({cmd: "touch done"}) }"#}))]
    #[case::loop_(r#"for (const d of ["a", "b"]) await tools.exec_command({cmd: "ls"})"#, CLAUDE, "exec", json!({"input": r#"for (const d of ["a", "b"]) await tools.exec_command({cmd: "ls"})"#}))]
    #[case::callback(r#"["a"].map(() => tools.exec_command({cmd: "ls"}))"#, CLAUDE, "exec", json!({"input": r#"["a"].map(() => tools.exec_command({cmd: "ls"}))"#}))]
    #[case::not_whole(r"await tools.exec_command({cmd: process.argv[2]})", CLAUDE, "exec", json!({"input": r"await tools.exec_command({cmd: process.argv[2]})"}))]
    fn codex_exec_scripts_are_carried_as_their_calls(
        #[case] script: &str,
        #[case] target: AnyHarness,
        #[case] want_name: &str,
        #[case] want_input: Value,
    ) {
        let call = ToolUse {
            id: ToolCallId::from("call_1".to_owned()),
            name: "exec".to_owned(),
            input: json!(script),
        };
        let result = ToolResult {
            call: call.id.clone(),
            output: json!("ok"),
            error: false,
        };
        let (ran_in, cwd) = (Path::new("/work"), Path::new("/here"));
        let (call, _) = carry(CODEX, target, &call, &result, ran_in, cwd, &mut CallIds::default());
        assert_eq!((call.name.as_str(), call.input), (want_name, want_input));
    }

    #[rstest]
    #[case::string(json!("3 passed"), "3 passed")]
    #[case::blocks(json!([{"type": "text", "text": "a"}, {"type": "input_text", "text": "b"}]), "a\nb")]
    #[case::uncaptured(Value::Null, UNCAPTURED_OUTPUT)]
    #[case::structured(json!({"exit": 0}), r#"{"exit":0}"#)]
    fn outputs_become_text(#[case] output: Value, #[case] want: &str) {
        let result = ToolResult {
            call: ToolCallId::from("c1".to_owned()),
            output,
            error: false,
        };
        assert_eq!(output_text(&result), want);
    }

    /// Kiro's shell tool reports its output under `Output:` with the command's exit code, which
    /// a result carried into Kiro gets too: 0, or 1 for a call that failed.
    #[rstest]
    #[case::ok(false, "Output:\n3 passed\n\n\nExit Code: 0")]
    #[case::failed(true, "Output:\n3 passed\n\n\nExit Code: 1")]
    fn a_shell_result_for_kiro_reports_an_exit_code(#[case] error: bool, #[case] want: &str) {
        let call = ToolUse {
            id: ToolCallId::from("call_1".to_owned()),
            name: "Bash".to_owned(),
            input: json!({"command": "cargo test"}),
        };
        let result = ToolResult {
            call: call.id.clone(),
            output: json!("3 passed"),
            error,
        };
        let (ran_in, cwd) = (Path::new("/work"), Path::new("/work"));
        let (_, carried) =
            carry(CLAUDE, KIRO, &call, &result, ran_in, cwd, &mut CallIds::default());
        assert_eq!(carried.output, json!(want));
        assert_eq!(carried.error, error);
        // A read's output is as it was.
        let read = ToolUse {
            id: ToolCallId::from("call_2".to_owned()),
            name: "Read".to_owned(),
            input: json!({"file_path": "/a.rs"}),
        };
        let (_, carried) =
            carry(CLAUDE, KIRO, &read, &result, ran_in, cwd, &mut CallIds::default());
        assert_eq!(carried.output, json!("3 passed"));
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
