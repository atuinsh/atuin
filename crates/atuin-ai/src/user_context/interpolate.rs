//! Parse `.atuin/ai-context.md` files and execute embedded commands.
//!
//! Two interpolation syntaxes are supported:
//!
//! **Inline:** `!`command`` — the `!` immediately before a code span triggers
//! execution. The entire `!`...`` span is replaced with the command's stdout.
//!
//! **Block:**
//! ````markdown
//! ```!
//! command
//! ```
//! ````
//! A fenced code block with `!` as the info string. The block body is executed
//! as a script and the entire fenced block is replaced with stdout.
//!
//! Regular code spans and fenced code blocks (without `!`) are left untouched.

use std::ops::Range;
use std::time::Duration;

use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};

/// A command to execute, with its byte range in the source for replacement.
#[derive(Debug)]
struct Command {
    /// Byte range in the source to replace (includes the `!` for inline, or
    /// the full ``` fence for blocks).
    range: Range<usize>,
    /// The command string to execute.
    body: String,
}

/// Maximum time for a single command.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(5);

/// Maximum bytes of stdout to capture from a single command.
const MAX_OUTPUT_BYTES: usize = 64 * 1024;

/// Parse a context file for interpolation commands.
fn parse_commands(source: &str) -> Vec<Command> {
    let parser = Parser::new_ext(source, Options::empty());
    let mut commands = Vec::new();

    // Block state: accumulate text across multiple Text events, finalize on End.
    let mut block_start: Option<usize> = None;
    let mut block_body = String::new();

    for (event, range) in parser.into_offset_iter() {
        match event {
            // Inline: !`command`
            Event::Code(code) if range.start > 0 && source.as_bytes()[range.start - 1] == b'!' => {
                commands.push(Command {
                    range: (range.start - 1)..range.end,
                    body: code.to_string(),
                });
            }

            // Block: ```! ... ```
            Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(info))) if info.as_ref() == "!" => {
                block_start = Some(range.start);
                block_body.clear();
            }
            Event::Text(text) if block_start.is_some() => {
                block_body.push_str(&text);
            }
            Event::End(TagEnd::CodeBlock) if block_start.is_some() => {
                let start = block_start.take().unwrap();
                let trimmed = block_body.trim();
                if !trimmed.is_empty() {
                    commands.push(Command {
                        range: start..range.end,
                        body: trimmed.to_string(),
                    });
                }
                block_body.clear();
            }

            _ => {}
        }
    }

    commands
}

/// Execute all commands in a context file and return the interpolated content.
///
/// Commands are executed in parallel. Failed commands are replaced with an
/// error marker so the AI has visibility into what went wrong.
pub(crate) async fn interpolate(source: &str, shell: &str) -> String {
    let commands = parse_commands(source);
    if commands.is_empty() {
        return source.to_string();
    }

    // Execute all commands in parallel.
    let mut handles = Vec::with_capacity(commands.len());
    for cmd in &commands {
        let shell = shell.to_string();
        let body = cmd.body.clone();
        handles.push(tokio::spawn(
            async move { run_command(&shell, &body).await },
        ));
    }

    // Collect results.
    let mut results = Vec::with_capacity(handles.len());
    for handle in handles {
        let output = match handle.await {
            Ok(output) => output,
            Err(e) => format!("[error: task panicked: {e}]"),
        };
        results.push(output);
    }

    // Rebuild the source, replacing command ranges with their output.
    // Commands are in source order from the parser, but let's sort to be safe.
    let mut replacements: Vec<(Range<usize>, &str)> = commands
        .iter()
        .zip(results.iter())
        .map(|(cmd, output)| (cmd.range.clone(), output.as_str()))
        .collect();
    replacements.sort_by_key(|(range, _)| range.start);

    let mut out = String::with_capacity(source.len());
    let mut cursor = 0;
    for (range, output) in &replacements {
        out.push_str(&source[cursor..range.start]);
        out.push_str(output);
        cursor = range.end;
    }
    out.push_str(&source[cursor..]);

    out
}

async fn run_command(shell: &str, body: &str) -> String {
    let result = tokio::time::timeout(
        COMMAND_TIMEOUT,
        tokio::process::Command::new(shell)
            .arg("-c")
            .arg(body)
            .output(),
    )
    .await;

    match result {
        Ok(Ok(output)) => {
            if output.status.success() {
                if output.stdout.len() > MAX_OUTPUT_BYTES {
                    let truncated = String::from_utf8_lossy(&output.stdout[..MAX_OUTPUT_BYTES]);
                    format!(
                        "{}\n[output truncated at {}KB]",
                        truncated.trim(),
                        MAX_OUTPUT_BYTES / 1024
                    )
                } else {
                    String::from_utf8_lossy(&output.stdout).trim().to_string()
                }
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                let code = output.status.code().unwrap_or(-1);
                format!("[error: exit code {code}: {stderr}]")
            }
        }
        Ok(Err(e)) => format!("[error: {e}]"),
        Err(_) => format!(
            "[error: command timed out after {}s]",
            COMMAND_TIMEOUT.as_secs()
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_inline_command() {
        let source = "Branch: !`git branch --show-current`";
        let cmds = parse_commands(source);
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].body, "git branch --show-current");
        assert_eq!(
            &source[cmds[0].range.clone()],
            "!`git branch --show-current`"
        );
    }

    #[test]
    fn parse_inline_double_backtick() {
        let source = r#"Host: !``echo `hostname` ``"#;
        let cmds = parse_commands(source);
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].body, "echo `hostname` ");
    }

    #[test]
    fn parse_block_command() {
        let source = "Before\n\n```!\necho hello\npython3 --version\n```\n\nAfter";
        let cmds = parse_commands(source);
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].body, "echo hello\npython3 --version");
    }

    #[test]
    fn regular_code_not_matched() {
        let source = "Normal `code span` and ```bash\necho hi\n```";
        let cmds = parse_commands(source);
        assert_eq!(cmds.len(), 0);
    }

    #[test]
    fn bang_not_adjacent_not_matched() {
        let source = "Exclaim! Then `code` here.";
        let cmds = parse_commands(source);
        // The `!` and backtick are separated by " Then ", not adjacent.
        assert_eq!(cmds.len(), 0);
    }

    #[test]
    fn mixed_content() {
        let source = "\
# Project Context

Branch: !`git branch --show-current`

Regular code: `not a command`

```!
echo $VIRTUAL_ENV
```

```bash
echo not interpolated
```

End.";
        let cmds = parse_commands(source);
        assert_eq!(cmds.len(), 2);
        assert_eq!(cmds[0].body, "git branch --show-current");
        assert_eq!(cmds[1].body, "echo $VIRTUAL_ENV");
    }

    #[tokio::test]
    async fn interpolate_replaces_inline_command() {
        let source = "Branch: !`echo main`";
        let result = interpolate(source, "sh").await;
        assert_eq!(result, "Branch: main");
    }

    #[tokio::test]
    async fn interpolate_replaces_block_command() {
        let source = "Before\n\n```!\necho hello world\n```\n\nAfter";
        let result = interpolate(source, "sh").await;
        assert_eq!(result, "Before\n\nhello world\n\nAfter");
    }

    #[tokio::test]
    async fn interpolate_preserves_non_command_content() {
        let source = "Just plain markdown with `code` and no bangs.";
        let result = interpolate(source, "sh").await;
        assert_eq!(result, source);
    }

    #[tokio::test]
    async fn interpolate_failed_command_shows_error() {
        let source = "Result: !`exit 1`";
        let result = interpolate(source, "sh").await;
        assert!(result.starts_with("Result: [error:"));
    }

    #[tokio::test]
    async fn interpolate_multiple_commands() {
        let source = "A: !`echo one` B: !`echo two`";
        let result = interpolate(source, "sh").await;
        assert_eq!(result, "A: one B: two");
    }
}
