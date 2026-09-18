//! Provenance for sessions Atuin writes into another agent's history.
//!
//! The first line of every such file carries an `atuin` object: the agent and session the
//! content came from, and how many bytes Atuin wrote. That one fact answers two questions.
//! A writer may replace the file only while it is still exactly that long; once the agent has
//! appended to it, it holds work that exists nowhere else. And ingest skips those bytes, which
//! are a copy of a session already in the store, while still reading what the agent added.

use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

use eyre::{Result, eyre};
use serde_json::{Value, json};

use crate::{Agent, Session};

const KEY: &str = "atuin";
/// `bytes` is written at a fixed width so that filling it in does not change the length it
/// reports.
const WIDTH: usize = 12;

/// Where a handed-off session came from, and how much of its file is Atuin's copy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Origin {
    pub agent: Agent,
    pub session_id: String,
    pub bytes: u64,
}

impl Origin {
    /// Read the stamp from a file's first line.
    #[must_use]
    pub fn from_header(header: &str) -> Option<Self> {
        let line: Value = serde_json::from_str(header).ok()?;
        let stamp = line.get(KEY)?;
        Some(Self {
            agent: stamp["agent"].as_str()?.parse().ok()?,
            session_id: stamp["session_id"].as_str()?.to_owned(),
            bytes: stamp["bytes"].as_str()?.parse().ok()?,
        })
    }

    #[must_use]
    pub fn of_file(path: &Path) -> Option<Self> {
        let mut header = String::new();
        BufReader::new(File::open(path).ok()?).read_line(&mut header).ok()?;
        Self::from_header(&header)
    }
}

/// Whether a writer may create or replace `path`: it is absent, or it is a handoff of ours that
/// the agent has not added to.
#[must_use]
pub fn writable(path: &Path) -> bool {
    match std::fs::metadata(path) {
        Err(_) => true,
        Ok(meta) => Origin::of_file(path).is_some_and(|origin| origin.bytes == meta.len()),
    }
}

/// Join JSONL `lines` into a file body whose first line is stamped with `session` as its origin.
pub fn stamp(mut lines: Vec<String>, session: &Session) -> Result<String> {
    let first = lines.first_mut().ok_or_else(|| eyre!("nothing to write"))?;
    let mut line: Value = serde_json::from_str(first)?;
    line[KEY] = json!({
        "agent": session.agent.to_string(),
        "session_id": session.session_id,
        "bytes": "0".repeat(WIDTH),
    });
    *first = line.to_string();
    let body = lines.join("\n") + "\n";
    let blank = format!(r#""bytes":"{}""#, "0".repeat(WIDTH));
    let filled = format!(r#""bytes":"{:0WIDTH$}""#, body.len());
    Ok(body.replacen(&blank, &filled, 1))
}

#[cfg(test)]
mod tests {
    use time::macros::datetime;

    use super::*;
    use crate::Tokens;

    #[test]
    fn a_stamp_reports_its_own_length_and_guards_rewrites() {
        let t = datetime!(2026-09-01 10:00 UTC);
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
            messages: 0,
            tool_calls: 0,
            threads: 0,
            tokens: Tokens::default(),
            last_stop: None,
        };
        let body =
            stamp(vec![r#"{"type":"session","id":"x"}"#.into(), r#"{"n":1}"#.into()], &session)
                .unwrap();
        let origin = Origin::from_header(body.lines().next().unwrap()).unwrap();
        assert_eq!(
            origin,
            Origin {
                agent: Agent::Codex,
                session_id: "th1".into(),
                bytes: body.len() as u64
            }
        );
        assert!(body.starts_with(r#"{"atuin":{"#) && body.ends_with("{\"n\":1}\n"));

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.jsonl");
        assert!(writable(&path)); // absent
        std::fs::write(&path, &body).unwrap();
        assert!(writable(&path)); // ours, untouched
        std::fs::write(&path, format!("{body}{{\"agent\":\"said more\"}}\n")).unwrap();
        assert!(!writable(&path)); // the agent continued it
        std::fs::write(&path, "{\"type\":\"user\"}\n").unwrap();
        assert!(!writable(&path)); // never ours
        assert!(stamp(Vec::new(), &session).is_err());
    }
}
