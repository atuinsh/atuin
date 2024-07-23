// Some wrappers around the Atuin history DB
// I'll probably use this to inform changes to the "upstream" client crate
// We also use Strings a bunch for errors. They're passed to the Tauri frontend,
// which requires that they be serializable.
// Can rework that in the future too, but my main concern is avoiding tauri limitations/reqs
// ending up in the main crate.

use serde::Serialize;
use sqlx::{sqlite::SqliteRow, Row};
use std::collections::HashMap;
use std::path::PathBuf;

use atuin_client::settings::{FilterMode, SearchMode};
use atuin_client::{
    database::{Context, Database, OptFilters, Sqlite},
    history::History,
};
use atuin_history::stats;

// useful for preprocessing data for the frontend
#[derive(Serialize, Debug)]
pub struct NameValue<T> {
    pub name: String,
    pub value: T,
}

#[derive(Serialize, Debug)]
pub struct GlobalStats {
    pub total_history: u64,

    pub daily: Vec<NameValue<u64>>,
    pub stats: Option<stats::Stats>,

    pub last_1d: u64,
    pub last_7d: u64,
    pub last_30d: u64,
}

#[derive(Serialize, Debug)]
pub struct UIHistory {
    pub id: String,
    /// When the command was run.
    pub timestamp: i128,
    /// How long the command took to run.
    pub duration: i64,
    /// The exit code of the command.
    pub exit: i64,
    /// The command that was run.
    pub command: String,
    /// The current working directory when the command was run.
    pub cwd: String,
    /// The session ID, associated with a terminal session.
    pub session: String,
    /// The hostname of the machine the command was run on.
    pub user: String,

    pub host: String,
}

impl From<History> for UIHistory {
    fn from(history: History) -> Self {
        let parts: Vec<String> = history.hostname.split(':').map(str::to_string).collect();

        let (host, user) = if parts.len() == 2 {
            (parts[0].clone(), parts[1].clone())
        } else {
            ("no-host".to_string(), "no-user".to_string())
        };

        let mac = format!("/Users/{}", user);
        let linux = format!("/home/{}", user);

        let cwd = history.cwd.replace(mac.as_str(), "~");
        let cwd = cwd.replace(linux.as_str(), "~");

        UIHistory {
            id: history.id.0,
            timestamp: history.timestamp.unix_timestamp_nanos(),
            duration: history.duration,
            exit: history.exit,
            command: history.command,
            session: history.session,
            host,
            user,
            cwd,
        }
    }
}

pub struct HistoryDB(Sqlite);

impl HistoryDB {
    pub async fn new(path: PathBuf, timeout: f64) -> Result<Self, String> {
        let sqlite = Sqlite::new(path, timeout)
            .await
            .map_err(|e| e.to_string())?;

        Ok(Self(sqlite))
    }

    pub async fn list(
        &self,
        offset: Option<u64>,
        limit: Option<usize>,
    ) -> Result<Vec<History>, String> {
        let query = if let Some(limit) = limit {
            sqlx::query("select * from history order by timestamp desc limit ?1 offset ?2")
                .bind(limit as i64)
                .bind(offset.unwrap_or(0) as i64)
        } else {
            sqlx::query("select * from history order by timestamp desc")
        };

        let history: Vec<History> = query
            .map(|row: SqliteRow| {
                History::from_db()
                    .id(row.get("id"))
                    .timestamp(
                        time::OffsetDateTime::from_unix_timestamp_nanos(
                            row.get::<i64, _>("timestamp") as i128,
                        )
                        .unwrap(),
                    )
                    .duration(row.get("duration"))
                    .exit(row.get("exit"))
                    .command(row.get("command"))
                    .cwd(row.get("cwd"))
                    .session(row.get("session"))
                    .hostname(row.get("hostname"))
                    .deleted_at(None)
                    .build()
                    .into()
            })
            .fetch_all(&self.0.pool)
            .await
            .map_err(|e| e.to_string())?;

        Ok(history)
    }

    pub async fn search(&self, offset: Option<u64>, query: &str) -> Result<Vec<UIHistory>, String> {
        let context = Context {
            session: "".to_string(),
            cwd: "".to_string(),
            host_id: "".to_string(),
            hostname: "".to_string(),
            git_root: None,
        };

        let filters = OptFilters {
            limit: Some(200),
            offset: offset.map(|offset| offset as i64),
            ..OptFilters::default()
        };

        let history = self
            .0
            .search(
                SearchMode::Fuzzy,
                FilterMode::Global,
                &context,
                query,
                filters,
            )
            .await
            .map_err(|e| e.to_string())?;

        let history = history
            .into_iter()
            .filter(|h| h.duration > 0)
            .map(|h| h.into())
            .collect();

        Ok(history)
    }

    pub async fn prefix_search(&self, query: &str) -> Result<Vec<UIHistory>, String> {
        let context = Context {
            session: "".to_string(),
            cwd: "".to_string(),
            host_id: "".to_string(),
            hostname: "".to_string(),
            git_root: None,
        };

        let filters = OptFilters {
            limit: Some(5),
            ..OptFilters::default()
        };

        let history = self
            .0
            .search(
                SearchMode::Prefix,
                FilterMode::Global,
                &context,
                query,
                filters,
            )
            .await
            .map_err(|e| e.to_string())?;

        let history = history
            .into_iter()
            .filter(|h| h.duration > 0)
            .map(|h| h.into())
            .collect();

        Ok(history)
    }

    pub async fn calendar(&self) -> Result<Vec<(String, u64)>, String> {
        let query = "select count(1) as count, strftime('%F', datetime(timestamp / 1000000000, 'unixepoch')) as day from history where timestamp > ((unixepoch() - 31536000) * 1000000000) group by day;";

        let calendar: Vec<(String, u64)> = sqlx::query(query)
            // safe to cast, count(x) is never < 0
            .map(|row: SqliteRow| {
                (
                    row.get::<String, _>("day"),
                    row.get::<i64, _>("count") as u64,
                )
            })
            .fetch_all(&self.0.pool)
            .await
            .map_err(|e| e.to_string())?;

        Ok(calendar)
    }

    pub async fn global_stats(&self) -> Result<GlobalStats, String> {
        let day_ago = time::OffsetDateTime::now_utc() - time::Duration::days(1);
        let day_ago = day_ago.unix_timestamp_nanos();

        let week_ago = time::OffsetDateTime::now_utc() - time::Duration::days(7);
        let week_ago = week_ago.unix_timestamp_nanos();

        let month_ago = time::OffsetDateTime::now_utc() - time::Duration::days(30);
        let month_ago = month_ago.unix_timestamp_nanos();

        // get the last 30 days of shell history
        let history: Vec<UIHistory> = sqlx::query("SELECT * FROM history WHERE timestamp > ?")
            .bind(month_ago as i64)
            .map(|row: SqliteRow| {
                History::from_db()
                    .id(row.get("id"))
                    .timestamp(
                        time::OffsetDateTime::from_unix_timestamp_nanos(
                            row.get::<i64, _>("timestamp") as i128,
                        )
                        .unwrap(),
                    )
                    .duration(row.get("duration"))
                    .exit(row.get("exit"))
                    .command(row.get("command"))
                    .cwd(row.get("cwd"))
                    .session(row.get("session"))
                    .hostname(row.get("hostname"))
                    .deleted_at(None)
                    .build()
                    .into()
            })
            .map(|h: History| h.into())
            .fetch_all(&self.0.pool)
            .await
            .map_err(|e| e.to_string())?;

        let total: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM history")
            .fetch_one(&self.0.pool)
            .await
            .map_err(|e| e.to_string())?;

        let mut day = 0;
        let mut week = 0;
        let mut month = 0;

        let mut daily = HashMap::new();
        let ymd = time::format_description::parse("[year]-[month]-[day]").unwrap();

        for i in history {
            if i.timestamp > day_ago {
                day += 1;
            }

            if i.timestamp > week_ago {
                week += 1;
            }

            if i.timestamp > month_ago {
                month += 1;

                // get the start of the day, as a unix timestamp
                let date = time::OffsetDateTime::from_unix_timestamp_nanos(i.timestamp)
                    .unwrap()
                    .format(&ymd)
                    .unwrap();

                daily.entry(date).and_modify(|v| *v += 1).or_insert(1);
            }
        }

        let mut daily: Vec<NameValue<u64>> = daily
            .into_iter()
            .map(|(k, v)| NameValue { name: k, value: v })
            .collect();
        daily.sort_by(|a, b| a.name.cmp(&b.name));

        Ok(GlobalStats {
            total_history: total.0 as u64,
            last_30d: month,
            last_7d: week,
            last_1d: day,
            daily,
            stats: None,
        })
    }
}
