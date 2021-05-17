use crate::error::Error;
use std::str::FromStr;

#[derive(Debug, Clone)]
pub enum SqliteJournalMode {
    Delete,
    Truncate,
    Persist,
    Memory,
    Wal,
    Off,
}

impl SqliteJournalMode {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            SqliteJournalMode::Delete => "DELETE",
            SqliteJournalMode::Truncate => "TRUNCATE",
            SqliteJournalMode::Persist => "PERSIST",
            SqliteJournalMode::Memory => "MEMORY",
            SqliteJournalMode::Wal => "WAL",
            SqliteJournalMode::Off => "OFF",
        }
    }
}

impl Default for SqliteJournalMode {
    fn default() -> Self {
        SqliteJournalMode::Wal
    }
}

impl FromStr for SqliteJournalMode {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Error> {
        Ok(match &*s.to_ascii_lowercase() {
            "delete" => SqliteJournalMode::Delete,
            "truncate" => SqliteJournalMode::Truncate,
            "persist" => SqliteJournalMode::Persist,
            "memory" => SqliteJournalMode::Memory,
            "wal" => SqliteJournalMode::Wal,
            "off" => SqliteJournalMode::Off,

            _ => {
                return Err(Error::Configuration(
                    format!("unknown value {:?} for `journal_mode`", s).into(),
                ));
            }
        })
    }
}
