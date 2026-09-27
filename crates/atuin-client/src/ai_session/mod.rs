pub mod model;
pub use model::*;
mod store;
pub use store::*;
mod database;
pub use database::*;

/// Tags harnesses wrap around text they inject into the user's turn. A user message opening with
/// one of these is not something a person typed; anything else starting with `<` (a pasted HTML
/// snippet, "`<Button>` doesn't render") is.
const INJECTED_TAGS: &[&str] = &[
    // Claude Code
    "system-reminder",
    "local-command-caveat",
    "local-command-stdout",
    "local-command-stderr",
    "bash-input",
    "bash-stdout",
    "bash-stderr",
    "task-notification",
    "fork-boilerplate",
    // Codex
    "environment_context",
    "user_instructions",
    "user_shell_command",
    "turn_aborted",
];

/// The part of a user message a person actually typed, or `None` for text the harness
/// injected on their behalf (see [`INJECTED_TAGS`], and Codex's AGENTS.md preamble). A
/// slash-command invocation is rendered as `/name args`.
#[must_use]
pub fn human_prompt(text: &str) -> Option<String> {
    fn tag<'a>(text: &'a str, name: &str) -> Option<&'a str> {
        let open = format!("<{name}>");
        let close = format!("</{name}>");
        let start = text.find(&open)? + open.len();
        let end = text[start..].find(&close)? + start;
        Some(text[start..end].trim())
    }

    let trimmed = text.trim_start();
    // Codex opens every session with the project's AGENTS.md as a user message.
    if trimmed.starts_with("# AGENTS.md instructions") {
        return None;
    }
    let opening = trimmed
        .strip_prefix('<')
        .and_then(|rest| rest.split(|c: char| c == '>' || c.is_whitespace()).next());
    if opening.is_some_and(|name| INJECTED_TAGS.contains(&name)) {
        return None;
    }
    let Some(name) = tag(trimmed, "command-name") else {
        return Some(text.to_owned());
    };
    let args = tag(trimmed, "command-args").unwrap_or_default();
    Some(if args.is_empty() {
        name.to_owned()
    } else {
        format!("{name} {args}")
    })
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::human_prompt;

    #[rstest]
    #[case::plain("fix the build", Some("fix the build"))]
    #[case::caveat("<local-command-caveat>Caveat: ...</local-command-caveat>", None)]
    #[case::reminder("<system-reminder>ctx</system-reminder>", None)]
    #[case::fork("<fork-boilerplate>\nYou are a worker fork.", None)]
    #[case::slash(
        "<command-message>loop</command-message>\n<command-name>/loop</command-name>\\
         n<command-args>test the mcp tools</command-args>",
        Some("/loop test the mcp tools")
    )]
    #[case::codex_agents_md("# AGENTS.md instructions for /repo\n\n<INSTRUCTIONS>", None)]
    #[case::user_typed_markup(
        "<Button> doesn't render in Safari",
        Some("<Button> doesn't render in Safari")
    )]
    #[case::pasted_html(
        "<div class=\"x\">hi</div> why is this blue",
        Some("<div class=\"x\">hi</div> why is this blue")
    )]
    #[case::codex_env("<environment_context>\n  <cwd>/x</cwd>\n</environment_context>", None)]
    #[case::bash_input("<bash-input>ls</bash-input>", None)]
    #[case::slash_no_args("<command-name>/login</command-name>", Some("/login"))]
    fn extracts_what_a_person_typed(#[case] text: &str, #[case] want: Option<&str>) {
        assert_eq!(human_prompt(text).as_deref(), want);
    }
}
