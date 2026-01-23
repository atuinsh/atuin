#![forbid(unsafe_code)]

pub mod calendar;
pub mod models;

use std::{
    collections::HashMap,
    fmt::{Debug, Display},
    ops::Range,
};

use self::{
    calendar::{TimePeriod, TimePeriodInfo},
    models::{History, NewHistory, NewSession, NewUser, Session, User},
};
use async_trait::async_trait;
use atuin_common::record::{EncryptedData, HostId, Record, RecordIdx, RecordStatus};
use serde::{Deserialize, Serialize};
use time::{Date, Duration, Month, OffsetDateTime, Time, UtcOffset};
use tracing::instrument;

#[derive(Debug)]
pub enum DbError {
    NotFound,
    Other(eyre::Report),
}

impl Display for DbError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl<T: std::error::Error + Into<time::error::Error>> From<T> for DbError {
    fn from(value: T) -> Self {
        DbError::Other(value.into().into())
    }
}

impl std::error::Error for DbError {}

pub type DbResult<T> = Result<T, DbError>;

#[derive(Debug, PartialEq)]
pub enum DbType {
    Postgres,
    Sqlite,
    Unknown,
}

#[derive(Clone, Deserialize, Serialize)]
pub struct DbSettings {
    pub db_uri: String,
    /// Optional URI for read replicas. If set, read-only queries will use this connection.
    pub read_db_uri: Option<String>,
}

impl DbSettings {
    pub fn db_type(&self) -> DbType {
        if self.db_uri.starts_with("postgres://") || self.db_uri.starts_with("postgresql://") {
            DbType::Postgres
        } else if self.db_uri.starts_with("sqlite://") {
            DbType::Sqlite
        } else {
            DbType::Unknown
        }
    }
}

fn redact_db_uri(uri: &str) -> String {
    url::Url::parse(uri)
        .map(|mut url| {
            let _ = url.set_password(Some("****"));
            url.to_string()
        })
        .unwrap_or_else(|_| uri.to_string())
}

// Do our best to redact passwords so they're not logged in the event of an error.
impl Debug for DbSettings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.db_type() == DbType::Postgres {
            let redacted_uri = redact_db_uri(&self.db_uri);
            let redacted_read_uri = self.read_db_uri.as_ref().map(|uri| redact_db_uri(uri));
            f.debug_struct("DbSettings")
                .field("db_uri", &redacted_uri)
                .field("read_db_uri", &redacted_read_uri)
                .finish()
        } else {
            f.debug_struct("DbSettings")
                .field("db_uri", &self.db_uri)
                .field("read_db_uri", &self.read_db_uri)
                .finish()
        }
    }
}

#[async_trait]
pub trait Database: Sized + Clone + Send + Sync + 'static {
    async fn new(settings: &DbSettings) -> DbResult<Self>;

    async fn get_session(&self, token: &str) -> DbResult<Session>;
    async fn get_session_user(&self, token: &str) -> DbResult<User>;
    async fn add_session(&self, session: &NewSession) -> DbResult<()>;

    async fn get_user(&self, username: &str) -> DbResult<User>;
    async fn get_user_session(&self, u: &User) -> DbResult<Session>;
    async fn add_user(&self, user: &NewUser) -> DbResult<i64>;

    async fn user_verified(&self, id: i64) -> DbResult<bool>;
    async fn verify_user(&self, id: i64) -> DbResult<()>;
    async fn user_verification_token(&self, id: i64) -> DbResult<String>;

    async fn update_user_password(&self, u: &User) -> DbResult<()>;

    async fn count_history(&self, user: &User) -> DbResult<i64>;
    async fn count_history_cached(&self, user: &User) -> DbResult<i64>;

    async fn delete_user(&self, u: &User) -> DbResult<()>;
    async fn delete_history(&self, user: &User, id: String) -> DbResult<()>;
    async fn deleted_history(&self, user: &User) -> DbResult<Vec<String>>;
    async fn delete_store(&self, user: &User) -> DbResult<()>;

    async fn add_records(&self, user: &User, record: &[Record<EncryptedData>]) -> DbResult<()>;
    async fn next_records(
        &self,
        user: &User,
        host: HostId,
        tag: String,
        start: Option<RecordIdx>,
        count: u64,
    ) -> DbResult<Vec<Record<EncryptedData>>>;

    // Return the tail record ID for each store, so (HostID, Tag, TailRecordID)
    async fn status(&self, user: &User) -> DbResult<RecordStatus>;

    async fn count_history_range(&self, user: &User, range: Range<OffsetDateTime>)
    -> DbResult<i64>;

    async fn list_history(
        &self,
        user: &User,
        created_after: OffsetDateTime,
        since: OffsetDateTime,
        host: &str,
        page_size: i64,
    ) -> DbResult<Vec<History>>;

    async fn add_history(&self, history: &[NewHistory]) -> DbResult<()>;

    async fn oldest_history(&self, user: &User) -> DbResult<History>;

    #[instrument(skip_all)]
    async fn calendar(
        &self,
        user: &User,
        period: TimePeriod,
        tz: UtcOffset,
    ) -> DbResult<HashMap<u64, TimePeriodInfo>> {
        let mut ret = HashMap::new();
        let iter: Box<dyn Iterator<Item = DbResult<(u64, Range<Date>)>> + Send> = match period {
            TimePeriod::Year => {
                // First we need to work out how far back to calculate. Get the
                // oldest history item
                let oldest = self
                    .oldest_history(user)
                    .await?
                    .timestamp
                    .to_offset(tz)
                    .year();
                let current_year = OffsetDateTime::now_utc().to_offset(tz).year();

                // All the years we need to get data for
                // The upper bound is exclusive, so include current +1
                let years = oldest..current_year + 1;

                Box::new(years.map(|year| {
                    let start = Date::from_calendar_date(year, time::Month::January, 1)?;
                    let end = Date::from_calendar_date(year + 1, time::Month::January, 1)?;

                    Ok((year as u64, start..end))
                }))
            }

            TimePeriod::Month { year } => {
                let months =
                    std::iter::successors(Some(Month::January), |m| Some(m.next())).take(12);

                Box::new(months.map(move |month| {
                    let start = Date::from_calendar_date(year, month, 1)?;
                    let days = start.month().length(year);
                    let end = start + Duration::days(days as i64);

                    Ok((month as u64, start..end))
                }))
            }

            TimePeriod::Day { year, month } => {
                let days = 1..month.length(year);
                Box::new(days.map(move |day| {
                    let start = Date::from_calendar_date(year, month, day)?;
                    let end = start
                        .next_day()
                        .ok_or_else(|| DbError::Other(eyre::eyre!("no next day?")))?;

                    Ok((day as u64, start..end))
                }))
            }
        };

        for x in iter {
            let (index, range) = x?;

            let start = range.start.with_time(Time::MIDNIGHT).assume_offset(tz);
            let end = range.end.with_time(Time::MIDNIGHT).assume_offset(tz);

            let count = self.count_history_range(user, start..end).await?;

            ret.insert(
                index,
                TimePeriodInfo {
                    count: count as u64,
                    hash: "".to_string(),
                },
            );
        }

        Ok(ret)
    }
}
