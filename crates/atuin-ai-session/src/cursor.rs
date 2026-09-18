//! Cursor: SQLite at `.../Cursor/User/globalStorage/state.vscdb`, table `cursorDiskKV`.
//! `composerData:<composer>` is a session, `bubbleId:<composer>:<bubble>` a message. There is no
//! cwd on either; the composer to workspace mapping lives in `workspaceStorage` and is skipped.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use eyre::Result;
use serde_json::Value;
use sqlx::{Connection, Row, SqliteConnection, sqlite::SqliteConnectOptions};
use time::OffsetDateTime;

use crate::ingest::{Stats, whole};
use crate::store::Store;
use crate::{
    Agent, FromNative, Message, Role, Tokens, ToolCall, cap_output, ts_millis, ts_rfc3339,
};

/// Cursor has no way to import a chat, so it implements only [`FromNative`]. Handing a session
/// to Cursor goes through `cursor-agent` with a transcript prompt instead.
pub struct Cursor;

impl FromNative for Cursor {
    const AGENT: Agent = Agent::Cursor;

    async fn ingest(store: &Store) -> Result<Stats> {
        let mut stats = Stats::default();
        let db = db_path();
        if db.is_file() {
            whole(store, &scan(&db).await?, &mut stats).await?;
        }
        Ok(stats)
    }
}

#[must_use]
pub fn db_path() -> PathBuf {
    let user = if cfg!(target_os = "macos") {
        atuin_common::utils::home_dir().join("Library/Application Support/Cursor/User")
    } else {
        atuin_common::utils::env_abspath("XDG_CONFIG_HOME")
            .unwrap_or_else(|| atuin_common::utils::home_dir().join(".config"))
            .join("Cursor/User")
    };
    user.join("globalStorage").join("state.vscdb")
}

/// Read every composer. Small enough to re-read whole; dedupe happens in the store.
pub async fn scan(db: &Path) -> Result<Vec<Message>> {
    let mut conn =
        SqliteConnection::connect_with(&SqliteConnectOptions::new().filename(db).read_only(true))
            .await?;
    let mut out = Vec::new();

    let mut created_at: HashMap<String, OffsetDateTime> = HashMap::new();
    for row in atuin_common::db::query(
        "select key, cast(value as text) from cursorDiskKV where key like 'composerData:%'",
    )
    .fetch_all(&mut conn)
    .await?
    {
        let key: String = row.get(0);
        let Ok(v) = serde_json::from_str::<Value>(&row.get::<String, _>(1)) else {
            continue;
        };
        let composer = key.trim_start_matches("composerData:").to_owned();
        let Some(ts) = v["createdAt"].as_i64().or(v["lastUpdatedAt"].as_i64()).and_then(ts_millis)
        else {
            continue;
        };
        if let Some(name) = v["name"].as_str().filter(|n| !n.trim().is_empty()) {
            let mut m = Message::new(Agent::Cursor, &composer, "title", ts, Role::Title);
            name.clone_into(&mut m.content);
            out.push(m);
        }
        created_at.insert(composer, ts);
    }

    for row in atuin_common::db::query(
        "select key, cast(value as text) from cursorDiskKV where key like 'bubbleId:%'",
    )
    .fetch_all(&mut conn)
    .await?
    {
        let key: String = row.get(0);
        let mut parts = key.splitn(3, ':');
        let (Some(_), Some(composer), Some(bubble)) = (parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        let Ok(v) = serde_json::from_str::<Value>(&row.get::<String, _>(1)) else {
            continue;
        };
        let role = match v["type"].as_u64() {
            Some(1) => Role::User,
            Some(2) => Role::Assistant,
            _ => continue,
        };
        let Some(ts) = v["createdAt"]
            .as_str()
            .and_then(ts_rfc3339)
            .or_else(|| created_at.get(composer).copied())
        else {
            continue;
        };
        let mut m = Message::new(Agent::Cursor, composer, bubble, ts, role);
        v["text"].as_str().unwrap_or_default().clone_into(&mut m.content);
        let tc = &v["tokenCount"];
        let (input, output) =
            (tc["inputTokens"].as_u64().unwrap_or(0), tc["outputTokens"].as_u64().unwrap_or(0));
        if input + output > 0 {
            m.tokens = Some(Tokens {
                input,
                output,
                ..Tokens::default()
            });
        }
        if let Some(tool) = v["toolFormerData"].as_object() {
            let id = tool.get("toolCallId").and_then(Value::as_str).unwrap_or(bubble).to_owned();
            m.tool_calls.push(ToolCall {
                id: id.clone(),
                name: tool.get("name").and_then(Value::as_str).unwrap_or_default().to_owned(),
                input: tool.get("rawArgs").and_then(Value::as_str).unwrap_or_default().to_owned(),
            });
            if let Some(result) = tool.get("result").filter(|r| !r.is_null()) {
                let mut r =
                    Message::new(Agent::Cursor, composer, format!("{bubble}#r"), ts, Role::Tool);
                r.tool_use_id = Some(id);
                r.is_error = tool.get("status").and_then(Value::as_str) == Some("error");
                r.content =
                    cap_output(result.as_str().map_or_else(|| result.to_string(), str::to_owned));
                out.push(r);
            }
        }
        if !m.content.trim().is_empty() || !m.tool_calls.is_empty() {
            out.push(m);
        }
    }
    Ok(out)
}
