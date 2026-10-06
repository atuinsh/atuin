//! Notes standing in for tool calls in a transcript written as text: what was called, on what
//! when the input is known (see [`tool_note`]). A transcript written back out from sync uses them
//! for the calls capture kept without their input, which no harness can replay.

use std::fmt::Write as _;

use serde_json::Value;

/// How much of an input a note shows, in characters.
const NOTE_INPUT: usize = 120;
/// How much of a tool's arguments a note for a tool without a mapping shows.
const NOTE_ARGS: usize = 160;
/// A piece of a turn written as text: something said, or a [note](tool_note) standing for
/// something done.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Part {
    Text(String),
    Note(String),
}

/// `parts` as one text: paragraphs apart, notes one to a line. The same note several times in a
/// row is written once, counted: `[ran a shell command] ×3`.
pub(crate) fn render(parts: &[Part]) -> String {
    let mut out = String::new();
    let mut last_note = false;
    let mut parts = parts.iter().peekable();
    while let Some(part) = parts.next() {
        let (text, note) = match part {
            Part::Text(text) => (text.trim(), false),
            Part::Note(note) => (note.as_str(), true),
        };
        if !out.is_empty() {
            out.push_str(if note && last_note {
                "\n"
            } else {
                "\n\n"
            });
        }
        out.push_str(text);
        if note {
            let mut repeats = 1;
            while parts.next_if(|next| matches!(next, Part::Note(n) if n == text)).is_some() {
                repeats += 1;
            }
            if repeats > 1 {
                let _ = write!(out, " ×{repeats}");
            }
        }
        last_note = note;
    }
    out
}

/// A note of one tool call: what was called on what, never how it turned out.
///
/// | Source | Tool | Note |
/// |---|---|---|
/// | Claude Code | `Bash` | ``ran `<command>` `` |
/// | | `Edit`, `MultiEdit`, `NotebookEdit` | ``edited `<file_path>` `` |
/// | | `Write` | ``wrote `<file_path>` `` |
/// | | `Read` | ``read `<file_path>` `` |
/// | | `Grep` | ``searched for `<pattern>` in `<path>` `` |
/// | | `Glob` | ``listed files matching `<pattern>` `` |
/// | | `Task`, `Agent` | `asked a subagent to <description>` |
/// | | `WebFetch`, `WebSearch` | ``fetched <url>``, ``searched the web for `<query>` `` |
/// | | `TodoWrite` | `updated its todo list` |
/// | Codex | `shell`, `shell_command`, `exec_command`, `local_shell` | ``ran `<command>` `` (an `argv` joined, `bash -lc` unwrapped) |
/// | | `apply_patch` | ``edited `<file>`, …`` / `added` / `deleted`, from the patch's headers |
/// | | `update_plan`, `view_image`, `web_search` | as `TodoWrite`, `Read`, `WebSearch` |
/// | opencode | `bash`, `edit`, `write`, `read`, `grep`, `glob`, `list`, `task`, `webfetch`, `todowrite`, `patch` | as Claude Code's (`filePath`, `patchText`) |
/// | Pi | `bash`, `edit`, `write`, `read`, `grep`, `find`, `ls` | as Claude Code's (`path`) |
/// | any | anything else | ``called `<name>` <compact arguments>`` |
///
/// Without the input (capture keeps none by default), each says only what was done: `ran a shell
/// command`, `edited a file`, `wrote a file`, `read a file`, `applied a patch`, `searched the
/// files`, `listed files`, `handed work to a subagent`, `fetched a web page`, `searched the
/// web`; and anything else ``called `<name>` ``. Inputs are cut to their first line and
/// 120 characters.
#[must_use]
pub fn tool_note(name: &str, input: &Value) -> String {
    // Codex keeps a function call's arguments as the JSON text the model wrote.
    let parsed;
    let input = match input.as_str().map(serde_json::from_str::<Value>) {
        Some(Ok(value @ Value::Object(_))) => {
            parsed = value;
            &parsed
        }
        _ => input,
    };
    let field = |keys: &[&str]| -> Option<String> {
        keys.iter().find_map(|k| match &input[*k] {
            Value::String(s) if !s.trim().is_empty() => Some(s.clone()),
            Value::Array(argv) if !argv.is_empty() => Some(command_line(argv)),
            _ => None,
        })
    };
    let path = || field(&["file_path", "filePath", "path", "notebook_path", "file"]);
    // What was done, on what when the input says (capture keeps none by default, only with
    // `ai.capture_tools` and in older records), else only what.
    let on = |what: Option<String>, detailed: &dyn Fn(&str) -> String, bare: &str| {
        Some(what.map_or_else(|| bare.to_owned(), |w| detailed(&w)))
    };
    let lower = name.to_ascii_lowercase();
    let note = match lower.as_str() {
        "bash" | "shell" | "shell_command" | "exec_command" | "local_shell" | "container.exec"
        | "unified_exec" => on(
            field(&["command", "cmd"])
                .or_else(|| input["action"]["command"].as_array().map(|a| command_line(a))),
            &|c| format!("ran {}", code(c)),
            "ran a shell command",
        ),
        "edit" | "multiedit" | "notebookedit" | "str_replace_based_edit_tool" | "str_replace" => {
            on(path(), &|p| format!("edited {}", code(p)), "edited a file")
        }
        "write" | "create" => on(path(), &|p| format!("wrote {}", code(p)), "wrote a file"),
        "read" | "view" | "cat" => on(path(), &|p| format!("read {}", code(p)), "read a file"),
        "view_image" => {
            on(path(), &|p| format!("looked at the image {}", code(p)), "looked at an image")
        }
        "apply_patch" | "patch" => {
            let text = input
                .as_str()
                .map(str::to_owned)
                .or_else(|| field(&["patchText", "patch", "input"]));
            Some(text.as_deref().and_then(patch_note).unwrap_or_else(|| "applied a patch".into()))
        }
        "grep" | "search" | "rg" => on(
            field(&["pattern", "query", "regex"]),
            &|pattern| match path() {
                Some(p) => format!("searched for {} in {}", code(pattern), code(&p)),
                None => format!("searched for {}", code(pattern)),
            },
            "searched the files",
        ),
        "glob" | "find" => on(
            field(&["pattern", "glob"]).or_else(path),
            &|p| format!("listed files matching {}", code(p)),
            "listed files",
        ),
        "ls" | "list" | "list_dir" => {
            on(path(), &|p| format!("listed {}", code(p)), "listed a directory")
        }
        "task" | "agent" | "subagent" | "spawn_agent" => on(
            field(&["description", "prompt", "message"]),
            &|d| format!("asked a subagent to {}", clip(d, NOTE_INPUT)),
            "handed work to a subagent",
        ),
        "webfetch" | "web_fetch" | "fetch" => {
            on(field(&["url"]), &|u| format!("fetched {u}"), "fetched a web page")
        }
        "websearch" | "web_search" => on(
            field(&["query"]).or_else(|| input["action"]["query"].as_str().map(str::to_owned)),
            &|q| format!("searched the web for {}", code(q)),
            "searched the web",
        ),
        "todowrite" | "todo_write" | "todoread" | "update_plan" => {
            Some("updated its todo list".to_owned())
        }
        _ => None,
    };
    let note = note.unwrap_or_else(|| {
        let args = match input {
            Value::Null => String::new(),
            Value::Object(map) if map.is_empty() => String::new(),
            Value::String(s) => format!(" {}", clip(s, NOTE_ARGS)),
            other => format!(" {}", clip(&other.to_string(), NOTE_ARGS)),
        };
        format!("called {}{args}", code(name))
    });
    format!("[{note}]")
}

/// `text` in backticks, cut short.
fn code(text: &str) -> String {
    format!("`{}`", clip(text, NOTE_INPUT))
}

/// A command given as its arguments, as a shell line: `bash -lc <script>` is its script.
pub(crate) fn command_line(argv: &[Value]) -> String {
    let words: Vec<&str> = argv.iter().filter_map(Value::as_str).collect();
    if let [shell, flag, script] = words.as_slice()
        && shell.rsplit('/').next().is_some_and(|s| matches!(s, "bash" | "sh" | "zsh"))
        && matches!(*flag, "-lc" | "-c")
    {
        return (*script).to_owned();
    }
    shlex::try_join(words.iter().copied()).unwrap_or_else(|_| words.join(" "))
}

/// What a patch (`*** Begin Patch` ... `*** End Patch`) did, by the files its headers name.
fn patch_note(patch: &str) -> Option<String> {
    let mut edited = Vec::new();
    let mut added = Vec::new();
    let mut deleted = Vec::new();
    for line in patch.lines() {
        let line = line.trim_end();
        if let Some(p) = line.strip_prefix("*** Update File: ") {
            edited.push(p);
        } else if let Some(p) = line.strip_prefix("*** Add File: ") {
            added.push(p);
        } else if let Some(p) = line.strip_prefix("*** Delete File: ") {
            deleted.push(p);
        }
    }
    let files = |verb: &str, paths: &[&str]| -> Option<String> {
        const SHOWN: usize = 3;
        if paths.is_empty() {
            return None;
        }
        let mut out = format!("{verb} ");
        let listed: Vec<String> = paths.iter().take(SHOWN).map(|p| code(p)).collect();
        out.push_str(&listed.join(", "));
        if paths.len() > SHOWN {
            let _ = write!(out, " and {} more", paths.len() - SHOWN);
        }
        Some(out)
    };
    let parts: Vec<String> =
        [files("edited", &edited), files("added", &added), files("deleted", &deleted)]
            .into_iter()
            .flatten()
            .collect();
    (!parts.is_empty()).then(|| parts.join("; "))
}

/// The first line of `text`, at most `max` characters, marked `…` where it was cut.
pub(crate) fn clip(text: &str, max: usize) -> String {
    let text = text.trim();
    let first = text.lines().next().unwrap_or_default().trim_end();
    let mut out: String = first.chars().take(max).collect();
    if out.len() < text.len() {
        out.push('…');
    }
    out
}

#[cfg(test)]
mod tests {
    use rstest::rstest;
    use serde_json::json;

    use super::*;

    #[rstest]
    #[case::claude_bash("Bash", json!({"command": "cargo test\n--nocapture", "description": "x"}), "[ran `cargo test…`]")]
    #[case::claude_write("Write", json!({"file_path": "a.rs", "content": "fn main() {}"}), "[wrote `a.rs`]")]
    #[case::claude_read("Read", json!({"file_path": "/w/src/lib.rs"}), "[read `/w/src/lib.rs`]")]
    #[case::claude_grep("Grep", json!({"pattern": "TODO", "path": "src"}), "[searched for `TODO` in `src`]")]
    #[case::claude_glob("Glob", json!({"pattern": "**/*.rs"}), "[listed files matching `**/*.rs`]")]
    #[case::claude_task("Task", json!({"description": "find the flaky test", "prompt": "..."}), "[asked a subagent to find the flaky test]")]
    #[case::codex_shell("shell", json!(r#"{"command":["bash","-lc","rg -n foo src"]}"#), "[ran `rg -n foo src`]")]
    #[case::codex_argv("shell", json!({"command": ["git", "log", "--oneline"]}), "[ran `git log --oneline`]")]
    #[case::codex_exec("exec_command", json!(r#"{"cmd":"ls -la"}"#), "[ran `ls -la`]")]
    #[case::codex_patch(
        "apply_patch",
        json!("*** Begin Patch\n*** Update File: src/a.rs\n@@\n-x\n+y\n*** Add File: src/b.rs\n+z\n*** End Patch"),
        "[edited `src/a.rs`; added `src/b.rs`]"
    )]
    #[case::codex_plan("update_plan", json!(r#"{"plan":[]}"#), "[updated its todo list]")]
    #[case::opencode_edit("edit", json!({"filePath": "/w/x.ts", "oldString": "a", "newString": "b"}), "[edited `/w/x.ts`]")]
    #[case::opencode_patch("patch", json!({"patchText": "*** Begin Patch\n*** Delete File: old.rs\n*** End Patch"}), "[deleted `old.rs`]")]
    #[case::pi_read("read", json!({"path": "README.md"}), "[read `README.md`]")]
    #[case::pi_ls("ls", json!({}), "[listed a directory]")]
    #[case::captured_bash("Bash", Value::Null, "[ran a shell command]")]
    #[case::captured_exec("exec_command", Value::Null, "[ran a shell command]")]
    #[case::captured_edit("edit", Value::Null, "[edited a file]")]
    #[case::captured_patch("apply_patch", Value::Null, "[applied a patch]")]
    #[case::captured_task("Task", Value::Null, "[handed work to a subagent]")]
    #[case::captured_other(
        "mcp__github__create_issue",
        Value::Null,
        "[called `mcp__github__create_issue`]"
    )]
    #[case::mcp("mcp__github__create_issue", json!({"title": "bug"}), r#"[called `mcp__github__create_issue` {"title":"bug"}]"#)]
    fn notes_name_the_call_and_its_key_input(
        #[case] name: &str,
        #[case] input: Value,
        #[case] want: &str,
    ) {
        assert_eq!(tool_note(name, &input), want);
    }

    #[rstest]
    fn long_inputs_are_cut() {
        let note = tool_note("Bash", &json!({"command": "x".repeat(500)}));
        assert_eq!(note.chars().count(), "[ran ``]".len() + NOTE_INPUT + 1);
        assert!(note.ends_with("…`]"));
        let note = tool_note("mystery", &json!({"blob": "y".repeat(500)}));
        assert!(note.chars().count() < NOTE_ARGS + 30, "{note}");
    }

    /// The same note several times in a row is written once, counted; text keeps its paragraphs.
    #[rstest]
    fn repeated_notes_are_counted() {
        let note = |n: &str| Part::Note(n.to_owned());
        let parts = [
            Part::Text("Looking.".to_owned()),
            note("[ran a shell command]"),
            note("[ran a shell command]"),
            note("[ran a shell command]"),
            note("[applied a patch]"),
            note("[ran a shell command]"),
            Part::Text("It's sparse.".to_owned()),
        ];
        assert_eq!(
            render(&parts),
            "Looking.\n\n[ran a shell command] ×3\n[applied a patch]\n[ran a shell \
             command]\n\nIt's sparse."
        );
    }
}
