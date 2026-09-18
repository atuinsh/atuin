//! Scoop one agent's local history into the store: dispatch plus the two read strategies.
//!
//! JSONL agents are tailed from a per-file byte offset, so a re-run reads only what was appended.
//! A trailing partial line (the agent is mid-write) is left for next time. SQLite agents are
//! re-read whole and deduped by the store.

use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::Path;

use eyre::Result;

use crate::handoff::Origin;
use crate::store::Store;
use crate::{Agent, FromNative, Message, claude_code, codex, cursor, opencode, pi};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Stats {
    /// Files or databases read.
    pub sources: usize,
    /// Messages parsed this run.
    pub read: usize,
    /// Messages new to the store.
    pub inserted: usize,
}

pub async fn ingest(store: &Store, agent: Agent) -> Result<Stats> {
    match agent {
        Agent::ClaudeCode => claude_code::ClaudeCode::ingest(store).await,
        Agent::Codex => codex::Codex::ingest(store).await,
        Agent::OpenCode => opencode::OpenCode::ingest(store).await,
        Agent::Cursor => cursor::Cursor::ingest(store).await,
        Agent::Pi => pi::Pi::ingest(store).await,
    }
}

/// Insert a whole scan.
pub(crate) async fn whole(store: &Store, messages: &[Message], stats: &mut Stats) -> Result<()> {
    stats.sources += 1;
    stats.read += messages.len();
    stats.inserted += store.insert(messages).await?;
    Ok(())
}

/// Read `path` from its saved offset, parse the complete lines, save the new offset.
/// `parse` gets the file's first line as a header, the new body, and the body's offset.
pub(crate) async fn tail(
    store: &Store,
    agent: Agent,
    path: &Path,
    stats: &mut Stats,
    parse: impl FnOnce(&str, &str, u64, &mut Vec<Message>),
) -> Result<()> {
    let saved = store.file_offset(agent, path.as_os_str()).await?;
    let mut file = BufReader::new(File::open(path)?);
    let len = file.get_ref().metadata()?.len();
    let mut header = String::new();
    file.read_line(&mut header)?;
    // A handoff of ours starts with a copy of a session already in the store: read only what
    // the agent added after it. A saved offset past the end means the file was replaced.
    let origin = Origin::from_header(&header);
    let saved = if saved > len {
        0
    } else {
        saved
    };
    let offset = saved.max(origin.as_ref().map_or(0, |o| o.bytes));
    if len <= offset {
        return Ok(());
    }
    file.seek(SeekFrom::Start(offset))?;
    let mut body = String::new();
    file.read_to_string(&mut body)?;
    // Only complete lines; a partial tail is still being written.
    let complete = body.rfind('\n').map_or(0, |i| i + 1);
    if complete == 0 {
        return Ok(());
    }
    let mut out = Vec::new();
    parse(header.trim_end(), &body[..complete], offset, &mut out);
    if let Some(origin) = origin {
        for m in &mut out {
            m.parent_agent = Some(origin.agent);
            m.parent_session_id = Some(origin.session_id.clone());
        }
    }
    stats.sources += 1;
    stats.read += out.len();
    stats.inserted += store.insert(&out).await?;
    store.set_file_offset(agent, path.as_os_str(), offset + complete as u64).await
}

#[cfg(test)]
mod tests {
    use time::macros::datetime;

    use super::*;
    use crate::{Role, Session, Tokens, handoff};

    /// A session handed to another agent is not ingested twice; what that agent adds is, as a
    /// session of its own that remembers where it came from.
    #[tokio::test]
    async fn a_handoff_is_skipped_and_its_continuation_is_linked() {
        let store = Store::in_memory().await.unwrap();
        let t = datetime!(2026-09-01 10:00 UTC);

        // The original, recorded in Codex and already in the store.
        let mut asked = Message::new(Agent::Codex, "th1", "10", t, Role::User);
        asked.content = "refactor sync".into();
        let mut answered = Message::new(Agent::Codex, "th1", "20", t, Role::Assistant);
        answered.content = "Done.".into();
        store.insert(&[asked, answered]).await.unwrap();

        // What a writer puts in Claude's history for it.
        let session = Session {
            agent: Agent::Codex,
            session_id: "th1".into(),
            parent_session_id: None,
            title: None,
            cwd: None,
            git_branch: None,
            model: None,
            started_at: t,
            ended_at: t,
            messages: 2,
            tool_calls: 0,
            threads: 0,
            tokens: Tokens::default(),
            last_stop: None,
        };
        let copied = [
            r#"{"type":"user","uuid":"w1","sessionId":"c1","timestamp":"2026-09-01T10:00:00Z","message":{"content":"refactor sync"}}"#,
            r#"{"type":"assistant","uuid":"w2","sessionId":"c1","timestamp":"2026-09-01T10:00:01Z","message":{"content":[{"type":"text","text":"Done."}]}}"#,
        ];
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("c1.jsonl");
        let body =
            handoff::stamp(copied.iter().map(|l| (*l).to_owned()).collect(), &session).unwrap();
        std::fs::write(&path, &body).unwrap();

        let parse = |_: &str, body: &str, base: u64, out: &mut Vec<Message>| {
            claude_code::parse(body, base, out);
        };
        let mut stats = Stats::default();
        tail(&store, Agent::ClaudeCode, &path, &mut stats, parse).await.unwrap();
        assert_eq!(stats, Stats::default()); // nothing but our own copy

        // Claude carries the conversation on.
        let said = r#"{"type":"user","uuid":"r1","sessionId":"c1","version":"2.1","timestamp":"2026-09-01T11:00:00Z","message":{"content":"now add tests"}}"#;
        std::fs::write(&path, format!("{body}{said}\n")).unwrap();
        tail(&store, Agent::ClaudeCode, &path, &mut stats, parse).await.unwrap();
        assert_eq!((stats.read, stats.inserted), (1, 1));

        let continuation = store.session_messages(Agent::ClaudeCode, "c1").await.unwrap();
        assert_eq!(continuation.len(), 1);
        assert_eq!(
            (continuation[0].parent_agent, continuation[0].parent_session_id.as_deref()),
            (Some(Agent::Codex), Some("th1"))
        );

        // Reopening the continuation elsewhere carries the whole conversation.
        let history = store.session_history(Agent::ClaudeCode, "c1").await.unwrap();
        let said: Vec<&str> = history.iter().map(|m| m.content.as_str()).collect();
        assert_eq!(said, ["refactor sync", "Done.", "now add tests"]);
        // And a session with no ancestors is just itself.
        assert_eq!(store.session_history(Agent::Codex, "th1").await.unwrap().len(), 2);

        // A file replaced by a shorter one is read again from the start of what is new.
        std::fs::write(&path, format!("{said_line}\n", said_line = copied[0])).unwrap();
        let mut again = Stats::default();
        tail(&store, Agent::ClaudeCode, &path, &mut again, parse).await.unwrap();
        assert_eq!(again.read, 1);
    }
}
