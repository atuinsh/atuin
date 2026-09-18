//! Reopen a stored session in any agent.
//!
//! [`reopen`] writes the session into the target agent's own history through its [`ToNative`]
//! and returns that agent's resume command. That works whether the target recorded the session
//! or not, and whether the original transcript still exists. Cursor cannot import a chat, so it
//! gets a transcript file and a prompt through `cursor-agent` instead.

use std::path::PathBuf;
use std::process::Command;

use eyre::Result;

use crate::claude_code::ClaudeCode;
use crate::codex::Codex;
use crate::opencode::OpenCode;
use crate::pi::Pi;
use crate::{Agent, Message, Role, Session, ToNative, clip, working_dir};

/// Longest tool input and result kept in a transcript, in chars.
const TOOL_INPUT_LEN: usize = 300;
const TOOL_RESULT_LEN: usize = 1_000;

/// The command that continues `session` in `target`, run in the session's directory.
pub async fn reopen(target: Agent, session: &Session, messages: &[Message]) -> Result<Command> {
    let mut cmd = match target {
        Agent::ClaudeCode => ClaudeCode::resume(&ClaudeCode::write(session, messages).await?),
        Agent::Codex => Codex::resume(&Codex::write(session, messages).await?),
        Agent::OpenCode => OpenCode::resume(&OpenCode::write(session, messages).await?),
        Agent::Pi => Pi::resume(&Pi::write(session, messages).await?),
        Agent::Cursor => {
            let path = transcript_path(session);
            std::fs::write(&path, transcript(session, messages))?;
            let mut c = Command::new("cursor-agent");
            c.arg(format!(
                "Continue a coding session that started in {}. Read the transcript at {} first, then \
                 carry on from where it left off. Do not redo work the transcript shows as finished.",
                session.agent,
                path.display()
            ));
            c
        }
    };
    cmd.current_dir(working_dir(session));
    Ok(cmd)
}

fn transcript_path(session: &Session) -> PathBuf {
    let safe: String = session
        .session_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c
            } else {
                '-'
            }
        })
        .collect();
    std::env::temp_dir().join(format!("atuin-handoff-{}-{safe}.md", session.agent))
}

/// The main thread of a session as markdown, tool traffic abridged.
#[must_use]
pub fn transcript(session: &Session, messages: &[Message]) -> String {
    let mut out = format!(
        "# {}\n\nAgent: {}\n",
        session.title.as_deref().unwrap_or("Untitled session"),
        session.agent
    );
    if let Some(cwd) = &session.cwd {
        out.push_str(&format!("Directory: {cwd}\n"));
    }
    if let Some(branch) = &session.git_branch {
        out.push_str(&format!("Branch: {branch}\n"));
    }
    out.push_str(&format!(
        "Started: {}\nLast activity: {}\n",
        session.started_at, session.ended_at
    ));

    for m in messages.iter().filter(|m| m.thread.is_none()) {
        match m.role {
            Role::Title => {}
            Role::User => out.push_str(&format!("\n## User\n\n{}\n", m.content)),
            Role::System => out.push_str(&format!("\n## System\n\n{}\n", m.content)),
            Role::Assistant => {
                out.push_str("\n## Assistant\n\n");
                if !m.content.is_empty() {
                    out.push_str(&m.content);
                    out.push('\n');
                }
                for tc in &m.tool_calls {
                    out.push_str(&format!(
                        "\n- Tool `{}`: `{}`\n",
                        tc.name,
                        clip(&tc.input, TOOL_INPUT_LEN)
                    ));
                }
            }
            Role::Tool => out.push_str(&format!(
                "\n### Tool result\n\n```\n{}\n```\n",
                clip(&m.content, TOOL_RESULT_LEN)
            )),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use time::macros::datetime;

    use super::*;
    use crate::{Tokens, ToolCall};

    #[tokio::test]
    async fn cursor_gets_a_transcript_prompt_in_the_session_directory() {
        let t = datetime!(2026-09-01 10:00 UTC);
        let session = Session {
            agent: Agent::Codex,
            session_id: "th1".into(),
            parent_session_id: None,
            title: Some("Refactor sync".into()),
            cwd: Some("/".into()),
            git_branch: Some("main".into()),
            model: None,
            started_at: t,
            ended_at: t,
            messages: 3,
            tool_calls: 1,
            threads: 0,
            tokens: Tokens::default(),
            last_stop: None,
        };
        let mut user = Message::new(Agent::Codex, "th1", "1", t, Role::User);
        user.content = "refactor sync".into();
        let mut call = Message::new(Agent::Codex, "th1", "2", t, Role::Assistant);
        call.tool_calls.push(ToolCall {
            id: "c1".into(),
            name: "exec".into(),
            input: "ls".into(),
        });
        let mut result = Message::new(Agent::Codex, "th1", "3", t, Role::Tool);
        result.content = "x".repeat(5_000);
        let mut side = Message::new(Agent::Codex, "th1", "4", t, Role::User);
        side.thread = Some("sub".into());
        side.content = "hidden".into();

        let cmd = reopen(Agent::Cursor, &session, &[user, call, result, side]).await.unwrap();
        assert_eq!(cmd.get_program(), "cursor-agent");
        assert_eq!(cmd.get_current_dir(), Some(std::path::Path::new("/")));
        let prompt = cmd.get_args().next().unwrap().to_string_lossy().into_owned();
        assert!(prompt.contains("started in codex"));
        let path = transcript_path(&session);
        assert!(prompt.contains(&path.display().to_string()));
        let md = std::fs::read_to_string(&path).unwrap();
        assert!(md.starts_with("# Refactor sync\n"));
        assert!(md.contains("## User\n\nrefactor sync"));
        assert!(md.contains("- Tool `exec`: `ls`"));
        assert!(md.contains(&format!("{}…", "x".repeat(TOOL_RESULT_LEN))));
        assert!(!md.contains("hidden"));
        let _ = std::fs::remove_file(path);
    }
}
