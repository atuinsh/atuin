// Some wrappers around the Atuin history DB
// I'll probably use this to inform changes to the "upstream" client crate
// We also use Strings a bunch for errors. They're passed to the Tauri frontend,
// which requires that they be serializable.
// Can rework that in the future too, but my main concern is avoiding tauri limitations/reqs
// ending up in the main crate.

use serde::Serialize;
use std::collections::HashMap;
use std::path::PathBuf;

use atuin_client::settings::{FilterMode, SearchMode};
use atuin_client::{
    database::{Context, Database, OptFilters, Sqlite},
    history::History,
};

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

    pub last_1d: u64,
    pub last_7d: u64,
    pub last_30d: u64,
}

#[derive(Serialize)]
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

pub fn to_ui_history(history: History) -> UIHistory {
    let parts: Vec<String> = history.hostname.split(":").map(str::to_string).collect();

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

pub struct HistoryDB(Sqlite);

impl HistoryDB {
    pub async fn new(path: PathBuf, timeout: f64) -> Result<Self, String> {
        let sqlite = Sqlite::new(path, timeout)
            .await
            .map_err(|e| e.to_string())?;

        Ok(Self(sqlite))
    }

    pub async fn list(&self, limit: Option<usize>, unique: bool) -> Result<Vec<UIHistory>, String> {
        let filters = vec![];

        // bit of a hack but provide an empty context
        // shell context makes _no sense_ in a GUI
        let context = Context {
            session: "".to_string(),
            cwd: "".to_string(),
            host_id: "".to_string(),
            hostname: "".to_string(),
            git_root: None,
        };

        let history = self
            .0
            .list(&filters, &context, limit, unique, false)
            .await
            .map_err(|e| e.to_string())?;

        let history = history
            .into_iter()
            .filter(|h| h.duration > 0)
            .map(to_ui_history)
            .collect();

        Ok(history)
    }

    pub async fn search(&self, query: &str) -> Result<Vec<UIHistory>, String> {
        let context = Context {
            session: "".to_string(),
            cwd: "".to_string(),
            host_id: "".to_string(),
            hostname: "".to_string(),
            git_root: None,
        };

        let filters = OptFilters {
            limit: Some(20),
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
            .map(to_ui_history)
            .collect();

        Ok(history)
    }

    pub async fn global_stats(&self) -> Result<GlobalStats, String> {
        let history = self.list(None, false).await?;

        let total = history.len();

        let day_ago = time::OffsetDateTime::now_utc() - time::Duration::days(1);
        let day_ago = day_ago.unix_timestamp_nanos();

        let week_ago = time::OffsetDateTime::now_utc() - time::Duration::days(7);
        let week_ago = week_ago.unix_timestamp_nanos();

        let month_ago = time::OffsetDateTime::now_utc() - time::Duration::days(30);
        let month_ago = month_ago.unix_timestamp_nanos();

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
            total_history: total as u64,
            last_30d: month,
            last_7d: week,
            last_1d: day,
            daily,
        })
    }
}
