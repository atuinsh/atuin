#![forbid(unsafe_code)]

pub mod calendar;
pub mod models;

use std::{
    collections::HashMap,
    fmt::{Debug, Display},
};

use self::{
    calendar::{TimePeriod, TimePeriodInfo},
    models::{History, NewHistory, NewSession, NewUser, Session, User},
};
use async_trait::async_trait;
use atuin_common::record::{EncryptedData, HostId, Record, RecordId, RecordIndex};
use serde::{de::DeserializeOwned, Serialize};
use time::{Date, Duration, Month, OffsetDateTime, PrimitiveDateTime, Time};
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
    async fn delete_user(&self, u: &User) -> DbResult<()>;

    async fn total_history(&self) -> DbResult<i64>;
    async fn count_history(&self, user: &User) -> DbResult<i64>;
    async fn count_history_cached(&self, user: &User) -> DbResult<i64>;

    async fn delete_history(&self, user: &User, id: String) -> DbResult<()>;
    async fn deleted_history(&self, user: &User) -> DbResult<Vec<String>>;

    async fn add_records(&self, user: &User, record: &[Record<EncryptedData>]) -> DbResult<()>;
    async fn next_records(
        &self,
        user: &User,
        host: HostId,
        tag: String,
        start: Option<RecordId>,
        count: u64,
    ) -> DbResult<Vec<Record<EncryptedData>>>;

    // Return the tail record ID for each store, so (HostID, Tag, TailRecordID)
    async fn tail_records(&self, user: &User) -> DbResult<RecordIndex>;

    async fn count_history_range(
        &self,
        user: &User,
        start: PrimitiveDateTime,
        end: PrimitiveDateTime,
    ) -> DbResult<i64>;

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

    /// Count the history for a given year
    #[instrument(skip_all)]
    async fn count_history_year(&self, user: &User, year: i32) -> DbResult<i64> {
        let start = Date::from_calendar_date(year, time::Month::January, 1)?;
        let end = Date::from_calendar_date(year + 1, time::Month::January, 1)?;

        let res = self
            .count_history_range(
                user,
                start.with_time(Time::MIDNIGHT),
                end.with_time(Time::MIDNIGHT),
            )
            .await?;
        Ok(res)
    }

    /// Count the history for a given month
    #[instrument(skip_all)]
    async fn count_history_month(&self, user: &User, year: i32, month: Month) -> DbResult<i64> {
        let start = Date::from_calendar_date(year, month, 1)?;
        let days = time::util::days_in_year_month(year, month);
        let end = start + Duration::days(days as i64);

        tracing::debug!("start: {}, end: {}", start, end);

        let res = self
            .count_history_range(
                user,
                start.with_time(Time::MIDNIGHT),
                end.with_time(Time::MIDNIGHT),
            )
            .await?;
        Ok(res)
    }

    /// Count the history for a given day
    #[instrument(skip_all)]
    async fn count_history_day(&self, user: &User, day: Date) -> DbResult<i64> {
        let end = day
            .next_day()
            .ok_or_else(|| DbError::Other(eyre::eyre!("no next day?")))?;

        let res = self
            .count_history_range(
                user,
                day.with_time(Time::MIDNIGHT),
                end.with_time(Time::MIDNIGHT),
            )
            .await?;
        Ok(res)
    }

    #[instrument(skip_all)]
    async fn calendar(
        &self,
        user: &User,
        period: TimePeriod,
        year: u64,
        month: Month,
    ) -> DbResult<HashMap<u64, TimePeriodInfo>> {
        // TODO: Support different timezones. Right now we assume UTC and
        // everything is stored as such. But it _should_ be possible to
        // interpret the stored date with a different TZ

        match period {
            TimePeriod::YEAR => {
                let mut ret = HashMap::new();
                // First we need to work out how far back to calculate. Get the
                // oldest history item
                let oldest = self.oldest_history(user).await?.timestamp.year();
                let current_year = OffsetDateTime::now_utc().year();

                // All the years we need to get data for
                // The upper bound is exclusive, so include current +1
                let years = oldest..current_year + 1;

                for year in years {
                    let count = self.count_history_year(user, year).await?;

                    ret.insert(
                        year as u64,
                        TimePeriodInfo {
                            count: count as u64,
                            hash: "".to_string(),
                        },
                    );
                }

                Ok(ret)
            }

            TimePeriod::MONTH => {
                let mut ret = HashMap::new();

                let months =
                    std::iter::successors(Some(Month::January), |m| Some(m.next())).take(12);
                for month in months {
                    let count = self.count_history_month(user, year as i32, month).await?;

                    ret.insert(
                        month as u64,
                        TimePeriodInfo {
                            count: count as u64,
                            hash: "".to_string(),
                        },
                    );
                }

                Ok(ret)
            }

            TimePeriod::DAY => {
                let mut ret = HashMap::new();

                for day in 1..time::util::days_in_year_month(year as i32, month) {
                    let count = self
                        .count_history_day(user, Date::from_calendar_date(year as i32, month, day)?)
                        .await?;

                    ret.insert(
                        day as u64,
                        TimePeriodInfo {
                            count: count as u64,
                            hash: "".to_string(),
                        },
                    );
                }

                Ok(ret)
            }
        }
    }
}
