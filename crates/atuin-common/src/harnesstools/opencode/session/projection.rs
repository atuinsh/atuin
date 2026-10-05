//! One session as opencode's projection holds it now (its `session`, `message` and `part`
//! tables), keyed as capture keys it: what `opencode --session` continues, for sync (see
//! [`crate::harnesstools::sync`]). The event log capture follows also holds what a revert
//! removed since; the projection does not.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use sqlx::{Connection, Row, Sqlite};
use xxhash_rust::xxh3::Xxh3;

use super::{
    Aggregate, Message, PartProjection, Reader, Reads, SqliteConnectOptions, SqliteConnection, text,
};
use crate::db::{query, query_as, query_scalar};

/// A session of opencode's projection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Projected {
    /// The directory opencode runs the session in.
    pub directory: PathBuf,
    /// When opencode created it.
    pub created: i64,
    /// A revert is pending: the next prompt removes the messages after it.
    pub reverting: bool,
    /// Its rows, in opencode's order (messages by creation time and id, each message's parts by
    /// id, then its failure): each capture row's source id.
    pub rows: Vec<String>,
    /// Its messages, in the same order.
    pub messages: Vec<ProjectedMessage>,
    /// A digest of every message and part row as read (id, creation time and data, in the same
    /// order): it changes whenever opencode's content does, including an edit that keeps a row's
    /// id.
    pub content: u64,
    /// When opencode last updated the session or any of its messages or parts (`time_updated`,
    /// milliseconds since the Unix epoch); `None` when that can't be read.
    pub updated: Option<i64>,
}

/// A message of the projection, as sync needs to know it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectedMessage {
    pub id: String,
    pub created: i64,
    pub assistant: bool,
    /// The user message an assistant message answers.
    pub parent: Option<String>,
    /// opencode is done writing it: a user message always; an assistant message once opencode
    /// stamps it `time.completed`, which it does when the reply ends.
    pub completed: bool,
}

impl Projected {
    /// Whether opencode is still writing the session's last reply: its last assistant message
    /// has no `time.completed`.
    #[must_use]
    pub fn replying(&self) -> bool {
        self.messages.iter().rev().find(|m| m.assistant).is_some_and(|m| !m.completed)
    }
}

/// Session `id` in the database `db`; `None` when its `session` table has no such session.
pub async fn projected(db: &Path, id: &str) -> Result<Option<Projected>, sqlx::Error> {
    let opts = SqliteConnectOptions::new()
        .filename(db)
        .read_only(true)
        .busy_timeout(Duration::from_secs(5));
    let mut conn = SqliteConnection::connect_with(&opts).await?;
    let found = read(&mut conn, db, id).await;
    let _ = conn.close().await;
    found
}

async fn read(
    conn: &mut SqliteConnection,
    db: &Path,
    id: &str,
) -> Result<Option<Projected>, sqlx::Error> {
    let Some(session) =
        query::<Sqlite>("SELECT directory, time_created FROM session WHERE id = ?1")
            .bind(id)
            .fetch_optional(&mut *conn)
            .await?
    else {
        return Ok(None);
    };
    let directory: String = session.try_get("directory")?;
    let created: i64 = session.try_get("time_created")?;
    // An opencode older than reverts has no such column: no revert.
    let revert: Option<String> =
        query_scalar::<Sqlite, Option<String>>("SELECT revert FROM session WHERE id = ?1")
            .bind(id)
            .fetch_one(&mut *conn)
            .await
            .ok()
            .flatten();
    // An opencode without these columns (or a part table that names no session) can't say.
    let updated = query_scalar::<Sqlite, Option<i64>>(
        "SELECT MAX(t) FROM (SELECT time_updated AS t FROM session WHERE id = ?1 UNION ALL SELECT \
         time_updated FROM message WHERE session_id = ?1 UNION ALL SELECT time_updated FROM part \
         WHERE session_id = ?1)",
    )
    .bind(id)
    .fetch_one(&mut *conn)
    .await
    .ok()
    .flatten();
    let listed = query::<Sqlite>(
        "SELECT id, time_created, data FROM message WHERE session_id = ?1 ORDER BY time_created, \
         id",
    )
    .bind(id)
    .fetch_all(&mut *conn)
    .await?;
    // Only the reader's `projected` is used, which reads nothing itself.
    let mut reader = Reader::from_start(
        Aggregate::Text(id.as_bytes().to_vec()),
        Arc::new(Reads::new(db.to_path_buf())),
    );
    let mut rows = Vec::new();
    let mut content = Xxh3::new();
    let mut messages = Vec::with_capacity(listed.len());
    for row in listed {
        let (Some(message), Some(data)) = (text(&row, "id")?, text(&row, "data")?) else {
            continue;
        };
        let created: Option<i64> = row.try_get_unchecked("time_created")?;
        let parts = query_as::<Sqlite, PartProjection>(
            "SELECT id, time_created, data FROM part WHERE message_id = ?1 ORDER BY id",
        )
        .bind(&message)
        .fetch_all(&mut *conn)
        .await?;
        digest(&mut content, b'm', Some(&message), created, Some(&data));
        for part in &parts {
            digest(&mut content, b'p', part.id.as_deref(), part.time_created, part.data.as_deref());
        }
        let info: serde_json::Value = serde_json::from_str(&data).unwrap_or_default();
        let assistant = info["role"] == "assistant";
        messages.push(ProjectedMessage {
            id: message.clone(),
            created: created.unwrap_or_default(),
            assistant,
            parent: info["parentID"].as_str().map(str::to_owned),
            completed: !assistant || !info["time"]["completed"].is_null(),
        });
        for delivered in reader.projected(&message, &data, parts).into_iter().flatten() {
            if let Some(id) = delivered.id() {
                rows.push(String::from(id));
            }
        }
    }
    Ok(Some(Projected {
        directory: PathBuf::from(directory),
        created,
        reverting: revert.is_some_and(|r| !r.trim().is_empty() && r != "null"),
        rows,
        messages,
        content: content.digest(),
        updated,
    }))
}

/// Feed one row into `hasher`, each field length-prefixed so that no two rows hash alike by
/// running into each other.
fn digest(hasher: &mut Xxh3, kind: u8, id: Option<&str>, created: Option<i64>, data: Option<&str>) {
    hasher.update(&[kind]);
    for field in [id, data] {
        let field = field.map_or(&[][..], str::as_bytes);
        hasher.update(&(field.len() as u64).to_le_bytes());
        hasher.update(field);
    }
    hasher.update(&created.unwrap_or(i64::MIN).to_le_bytes());
}
