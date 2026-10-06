//! What a tool call touches, whichever harness made it: the command it runs, and the files it
//! reads and writes. What capture keeps of a call can depend on it (a command history would
//! ignore, a read of `.env`).

use serde_json::{Map, Value};

use crate::harnesstools::note::command_line;

/// What a tool call touches.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Access {
    /// The command line a shell tool runs.
    pub command: Option<String>,
    /// Where it runs it, when the call says (Codex's `workdir`).
    pub workdir: Option<String>,
    /// Files the call reads or searches.
    pub reads: Vec<String>,
    /// Files the call writes, edits or patches.
    pub writes: Vec<String>,
}

/// The input keys a file's path is given under.
const PATH_KEYS: &[&str] = &["file_path", "filePath", "path", "notebook_path", "filename", "file"];
const PATHS_KEYS: &[&str] = &["paths", "file_paths", "filePaths", "files"];

impl Access {
    /// What the call `name`, with `input`, touches. Tools are known by name, case aside and
    /// without an MCP server's prefix (`mcp__fs__write_file`): shells by the command they run, and
    /// a writer by the paths it's given; any other tool given a path reads it (a read, a search,
    /// a listing).
    #[must_use]
    pub fn of(name: &str, input: &Value) -> Self {
        let lower = name.to_ascii_lowercase();
        let tool = lower.rsplit("__").next().unwrap_or(&lower);
        let mut access = Self::default();
        if tool == "apply_patch" {
            let patch = match input {
                Value::String(text) => object(input)
                    .and_then(|o| o.get("input").and_then(Value::as_str).map(str::to_owned))
                    .or_else(|| Some(text.clone())),
                _ => object(input).and_then(|o| {
                    ["input", "patch"].iter().find_map(|k| o.get(*k)?.as_str().map(str::to_owned))
                }),
            };
            access.writes = patch.as_deref().map(patched).unwrap_or_default();
            return access;
        }
        let Some(input) = object(input) else {
            return access;
        };
        if matches!(
            tool,
            "bash"
                | "shell"
                | "shell_command"
                | "exec_command"
                | "local_shell"
                | "container.exec"
                | "run_terminal_cmd"
                | "execute_command"
                | "run_command"
        ) {
            let command = match input.get("command").or_else(|| input.get("cmd")) {
                Some(Value::String(line)) => Some(line.clone()),
                Some(Value::Array(argv)) => Some(command_line(argv)),
                _ => None,
            };
            // Codex patches through its shell too (`["apply_patch", "*** Begin Patch ..."]`), and
            // any shell writes where it redirects to (`printf ... > .env`).
            access.writes = command
                .as_deref()
                .map(|command| patched(command).into_iter().chain(redirected(command)).collect())
                .unwrap_or_default();
            access.command = command;
            access.workdir = ["workdir", "working_directory", "cwd"]
                .iter()
                .find_map(|k| input.get(*k)?.as_str().map(str::to_owned));
            return access;
        }
        // opencode's patch tool names its files only in its patch.
        if let Some(patch) =
            ["patchText", "patch_text", "patch"].iter().find_map(|k| input.get(*k)?.as_str())
        {
            access.writes = patched(patch);
            return access;
        }
        let paths: Vec<String> = PATH_KEYS
            .iter()
            .filter_map(|k| input.get(*k)?.as_str())
            .chain(
                PATHS_KEYS
                    .iter()
                    .filter_map(|k| input.get(*k)?.as_array())
                    .flatten()
                    .filter_map(Value::as_str),
            )
            .map(str::to_owned)
            .collect();
        let writes = matches!(
            tool,
            "write"
                | "edit"
                | "multiedit"
                | "notebookedit"
                | "write_file"
                | "edit_file"
                | "create_file"
                | "str_replace_editor"
                | "str_replace_based_edit_tool"
                | "patch"
                | "create"
                | "apply_diff"
        );
        if writes {
            access.writes = paths;
        } else {
            access.reads = paths;
        }
        access
    }
}

/// A call's input as an object: Codex keeps a function call's arguments as the JSON text the
/// model wrote.
fn object(input: &Value) -> Option<Map<String, Value>> {
    match input {
        Value::Object(map) => Some(map.clone()),
        Value::String(text) => serde_json::from_str(text).ok(),
        _ => None,
    }
}

/// The files a shell `command` writes to by redirecting its output (`> .env`, `>> log`,
/// `&> out`) or through `tee`.
fn redirected(command: &str) -> Vec<String> {
    let words = shlex::split(command)
        .unwrap_or_else(|| command.split_whitespace().map(str::to_owned).collect());
    let mut files = Vec::new();
    let mut words = words.iter().map(String::as_str);
    let mut teeing = false;
    while let Some(word) = words.next() {
        if matches!(word, "|" | "||" | "&&" | ";" | "&") {
            teeing = false;
            continue;
        }
        if teeing && !word.starts_with('-') {
            files.push(word.to_owned());
            continue;
        }
        if word.rsplit('/').next() == Some("tee") {
            teeing = true;
            continue;
        }
        // `> file`, `>file`, `2>>file`, `&>file`, `echo x>file`: what follows the last `>`, else
        // the next word. Not a duplicated descriptor (`2>&1`).
        if let Some(at) = word.rfind('>') {
            let target = word[at + 1..].trim_start_matches('|');
            let target = if target.is_empty() {
                words.next().unwrap_or_default()
            } else {
                target
            };
            if !target.is_empty() && !target.starts_with('&') {
                files.push(target.to_owned());
            }
        }
    }
    files
}

/// The files a patch (`*** Begin Patch` ... `*** End Patch`) names in its headers.
fn patched(patch: &str) -> Vec<String> {
    patch
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            ["*** Update File: ", "*** Add File: ", "*** Delete File: ", "*** Move to: "]
                .iter()
                .find_map(|header| line.strip_prefix(header))
        })
        .map(|path| path.trim().to_owned())
        .collect()
}

#[cfg(test)]
mod tests {
    use rstest::rstest;
    use serde_json::json;

    use super::*;

    fn shell(command: &str, workdir: Option<&str>) -> Access {
        Access {
            command: Some(command.to_owned()),
            workdir: workdir.map(str::to_owned),
            ..Access::default()
        }
    }

    fn reads(paths: &[&str]) -> Access {
        Access {
            reads: paths.iter().map(|&p| p.to_owned()).collect(),
            ..Access::default()
        }
    }

    fn writes(paths: &[&str]) -> Access {
        Access {
            writes: paths.iter().map(|&p| p.to_owned()).collect(),
            ..Access::default()
        }
    }

    #[rstest]
    #[case::claude_bash("Bash", json!({"command": "cat .env"}), shell("cat .env", None))]
    #[case::codex_argv("shell", json!({"command": ["bash", "-lc", "ls"], "workdir": "/w"}), shell("ls", Some("/w")))]
    #[case::codex_exec("exec_command", json!(r#"{"cmd":"git status","workdir":"/w"}"#), shell("git status", Some("/w")))]
    #[case::claude_read("Read", json!({"file_path": "/w/.env"}), reads(&["/w/.env"]))]
    #[case::opencode_read("read", json!({"filePath": "/w/.env"}), reads(&["/w/.env"]))]
    #[case::grep("Grep", json!({"pattern": "x", "path": "/w"}), reads(&["/w"]))]
    #[case::claude_write("Write", json!({"file_path": "/w/.env", "content": "A=1"}), writes(&["/w/.env"]))]
    #[case::pi_edit("edit", json!({"path": "a.rs", "edits": []}), writes(&["a.rs"]))]
    #[case::mcp_write("mcp__fs__write_file", json!({"path": "a.txt", "content": ""}), writes(&["a.txt"]))]
    #[case::patch_freeform("apply_patch", json!("*** Begin Patch\n*** Update File: .env\n@@\n-A=1\n+A=2\n*** End Patch"), writes(&[".env"]))]
    #[case::patch_object("apply_patch", json!({"input": "*** Begin Patch\n*** Add File: k.pem\n*** End Patch"}), writes(&["k.pem"]))]
    #[case::patch_by_shell("shell", json!({"command": ["apply_patch", "*** Begin Patch\n*** Delete File: a.rs\n*** End Patch"]}), Access { writes: vec!["a.rs".into()], ..shell("apply_patch '*** Begin Patch\n*** Delete File: a.rs\n*** End Patch'", None) })]
    #[case::redirect("Bash", json!({"command": "printf '%s' 'pw' > .env"}), Access { writes: vec![".env".into()], ..shell("printf '%s' 'pw' > .env", None) })]
    #[case::redirect_attached("Bash", json!({"command": "echo A=1>>.env.local"}), Access { writes: vec![".env.local".into()], ..shell("echo A=1>>.env.local", None) })]
    #[case::heredoc("Bash", json!({"command": "cat > .env <<EOF\nA=1\nEOF"}), Access { writes: vec![".env".into()], ..shell("cat > .env <<EOF\nA=1\nEOF", None) })]
    #[case::tee("Bash", json!({"command": "echo A=1 | tee -a .env"}), Access { writes: vec![".env".into()], ..shell("echo A=1 | tee -a .env", None) })]
    #[case::descriptor("Bash", json!({"command": "make 2>&1"}), shell("make 2>&1", None))]
    #[case::opencode_patch("patch", json!({"patchText": "*** Begin Patch\n*** Update File: .env\n+A=1\n*** End Patch"}), writes(&[".env"]))]
    #[case::other("WebSearch", json!({"query": "rust"}), Access::default())]
    #[case::uncaptured("Read", Value::Null, Access::default())]
    fn what_a_call_touches(#[case] name: &str, #[case] input: Value, #[case] want: Access) {
        assert_eq!(Access::of(name, &input), want);
    }
}
