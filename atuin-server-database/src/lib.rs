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
use atuin_common::utils::get_days_from_month;
use chrono::{Datelike, TimeZone};
use chronoutil::RelativeDuration;
use serde::{de::DeserializeOwned, Serialize};
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

    async fn count_history(&self, user: &User) -> DbResult<i64>;
    async fn count_history_cached(&self, user: &User) -> DbResult<i64>;

    async fn delete_history(&self, user: &User, id: String) -> DbResult<()>;
    async fn deleted_history(&self, user: &User) -> DbResult<Vec<String>>;

    async fn count_history_range(
        &self,
        user: &User,
        start: chrono::NaiveDateTime,
        end: chrono::NaiveDateTime,
    ) -> DbResult<i64>;

    async fn list_history(
        &self,
        user: &User,
        created_after: chrono::NaiveDateTime,
        since: chrono::NaiveDateTime,
        host: &str,
        page_size: i64,
    ) -> DbResult<Vec<History>>;

    async fn add_history(&self, history: &[NewHistory]) -> DbResult<()>;

    async fn oldest_history(&self, user: &User) -> DbResult<History>;

    /// Count the history for a given year
    #[instrument(skip_all)]
    async fn count_history_year(&self, user: &User, year: i32) -> DbResult<i64> {
        let start = chrono::Utc.ymd(year, 1, 1).and_hms_nano(0, 0, 0, 0);
        let end = start + RelativeDuration::years(1);

        let res = self
            .count_history_range(user, start.naive_utc(), end.naive_utc())
            .await?;
        Ok(res)
    }

    /// Count the history for a given month
    #[instrument(skip_all)]
    async fn count_history_month(&self, user: &User, month: chrono::NaiveDate) -> DbResult<i64> {
        let start = chrono::Utc
            .ymd(month.year(), month.month(), 1)
            .and_hms_nano(0, 0, 0, 0);

        // ofc...
        let end = if month.month() < 12 {
            chrono::Utc
                .ymd(month.year(), month.month() + 1, 1)
                .and_hms_nano(0, 0, 0, 0)
        } else {
            chrono::Utc
                .ymd(month.year() + 1, 1, 1)
                .and_hms_nano(0, 0, 0, 0)
        };

        tracing::debug!("start: {}, end: {}", start, end);

        let res = self
            .count_history_range(user, start.naive_utc(), end.naive_utc())
            .await?;
        Ok(res)
    }

    /// Count the history for a given day
    #[instrument(skip_all)]
    async fn count_history_day(&self, user: &User, day: chrono::NaiveDate) -> DbResult<i64> {
        let start = chrono::Utc
            .ymd(day.year(), day.month(), day.day())
            .and_hms_nano(0, 0, 0, 0);
        let end = chrono::Utc
            .ymd(day.year(), day.month(), day.day() + 1)
            .and_hms_nano(0, 0, 0, 0);

        let res = self
            .count_history_range(user, start.naive_utc(), end.naive_utc())
            .await?;
        Ok(res)
    }

    #[instrument(skip_all)]
    async fn calendar(
        &self,
        user: &User,
        period: TimePeriod,
        year: u64,
        month: u64,
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
                let current_year = chrono::Utc::now().year();

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

                for month in 1..13 {
                    let count = self
                        .count_history_month(
                            user,
                            chrono::Utc.ymd(year as i32, month, 1).naive_utc(),
                        )
                        .await?;

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

                for day in 1..get_days_from_month(year as i32, month as u32) {
                    let count = self
                        .count_history_day(
                            user,
                            chrono::Utc
                                .ymd(year as i32, month as u32, day as u32)
                                .naive_utc(),
                        )
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
