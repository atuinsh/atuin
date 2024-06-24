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
use serde::{de::DeserializeOwned, Serialize};
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

#[async_trait]
pub trait Database: Sized + Clone + Send + Sync + 'static {
    type Settings: Debug + Clone + DeserializeOwned + Serialize + Send + Sync + 'static;
    async fn new(settings: &Self::Settings) -> DbResult<Self>;

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

    async fn total_history(&self) -> DbResult<i64>;
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
                    let days = time::util::days_in_year_month(year, month);
                    let end = start + Duration::days(days as i64);

                    Ok((month as u64, start..end))
                }))
            }

            TimePeriod::Day { year, month } => {
                let days = 1..time::util::days_in_year_month(year, month);
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
