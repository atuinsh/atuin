use std::collections::HashMap;

use async_trait::async_trait;
use chrono::{Datelike, TimeZone};
use chronoutil::RelativeDuration;
use sqlx::{postgres::PgPoolOptions, Result};

use sqlx::Row;

use tracing::{debug, instrument, warn};

use super::{
    calendar::{TimePeriod, TimePeriodInfo},
    models::{History, NewHistory, NewSession, NewUser, Session, User},
};
use crate::settings::Settings;
use crate::settings::HISTORY_PAGE_SIZE;

use atuin_common::utils::get_days_from_month;

#[async_trait]
pub trait Database {
    async fn get_session(&self, token: &str) -> Result<Session>;
    async fn get_session_user(&self, token: &str) -> Result<User>;
    async fn add_session(&self, session: &NewSession) -> Result<()>;

    async fn get_user(&self, username: &str) -> Result<User>;
    async fn get_user_session(&self, u: &User) -> Result<Session>;
    async fn add_user(&self, user: &NewUser) -> Result<i64>;

    async fn count_history(&self, user: &User) -> Result<i64>;
    async fn count_history_cached(&self, user: &User) -> Result<i64>;

    async fn delete_history(&self, user: &User, id: String) -> Result<()>;
    async fn deleted_history(&self, user: &User) -> Result<Vec<String>>;

    async fn count_history_range(
        &self,
        user: &User,
        start: chrono::NaiveDateTime,
        end: chrono::NaiveDateTime,
    ) -> Result<i64>;
    async fn count_history_day(&self, user: &User, date: chrono::NaiveDate) -> Result<i64>;
    async fn count_history_month(&self, user: &User, date: chrono::NaiveDate) -> Result<i64>;
    async fn count_history_year(&self, user: &User, year: i32) -> Result<i64>;

    async fn list_history(
        &self,
        user: &User,
        created_after: chrono::NaiveDateTime,
        since: chrono::NaiveDateTime,
        host: &str,
    ) -> Result<Vec<History>>;

    async fn add_history(&self, history: &[NewHistory]) -> Result<()>;

    async fn oldest_history(&self, user: &User) -> Result<History>;

    async fn calendar(
        &self,
        user: &User,
        period: TimePeriod,
        year: u64,
        month: u64,
    ) -> Result<HashMap<u64, TimePeriodInfo>>;
}

#[derive(Clone)]
pub struct Postgres {
    pool: sqlx::Pool<sqlx::postgres::Postgres>,
    settings: Settings,
}

impl Postgres {
    pub async fn new(settings: Settings) -> Result<Self> {
        let pool = PgPoolOptions::new()
            .max_connections(100)
            .connect(settings.db_uri.as_str())
            .await?;

        sqlx::migrate!("./migrations").run(&pool).await?;

        Ok(Self { pool, settings })
    }
}

#[async_trait]
impl Database for Postgres {
    #[instrument(skip_all)]
    async fn get_session(&self, token: &str) -> Result<Session> {
        sqlx::query_as::<_, Session>("select id, user_id, token from sessions where token = $1")
            .bind(token)
            .fetch_one(&self.pool)
            .await
    }

    #[instrument(skip_all)]
    async fn get_user(&self, username: &str) -> Result<User> {
        sqlx::query_as::<_, User>(
            "select id, username, email, password from users where username = $1",
        )
        .bind(username)
        .fetch_one(&self.pool)
        .await
    }

    #[instrument(skip_all)]
    async fn get_session_user(&self, token: &str) -> Result<User> {
        sqlx::query_as::<_, User>(
            "select users.id, users.username, users.email, users.password from users
            inner join sessions
            on users.id = sessions.user_id
            and sessions.token = $1",
        )
        .bind(token)
        .fetch_one(&self.pool)
        .await
    }

    #[instrument(skip_all)]
    async fn count_history(&self, user: &User) -> Result<i64> {
        // The cache is new, and the user might not yet have a cache value.
        // They will have one as soon as they post up some new history, but handle that
        // edge case.

        let res: (i64,) = sqlx::query_as(
            "select count(1) from history
            where user_id = $1",
        )
        .bind(user.id)
        .fetch_one(&self.pool)
        .await?;

        Ok(res.0)
    }

    #[instrument(skip_all)]
    async fn count_history_cached(&self, user: &User) -> Result<i64> {
        let res: (i32,) = sqlx::query_as(
            "select total from total_history_count_user
            where user_id = $1",
        )
        .bind(user.id)
        .fetch_one(&self.pool)
        .await?;

        Ok(res.0 as i64)
    }

    async fn delete_history(&self, user: &User, id: String) -> Result<()> {
        sqlx::query(
            "update history
            set deleted_at = $3
            where user_id = $1
            and client_id = $2
            and deleted_at is null", // don't just keep setting it
        )
        .bind(user.id)
        .bind(id)
        .bind(chrono::Utc::now().naive_utc())
        .fetch_all(&self.pool)
        .await?;

        Ok(())
    }

    #[instrument(skip_all)]
    async fn deleted_history(&self, user: &User) -> Result<Vec<String>> {
        // The cache is new, and the user might not yet have a cache value.
        // They will have one as soon as they post up some new history, but handle that
        // edge case.

        let res = sqlx::query(
            "select client_id from history
            where user_id = $1
            and deleted_at is not null",
        )
        .bind(user.id)
        .fetch_all(&self.pool)
        .await?;

        let res = res
            .iter()
            .map(|row| row.get::<String, _>("client_id"))
            .collect();

        Ok(res)
    }

    #[instrument(skip_all)]
    async fn count_history_range(
        &self,
        user: &User,
        start: chrono::NaiveDateTime,
        end: chrono::NaiveDateTime,
    ) -> Result<i64> {
        let res: (i64,) = sqlx::query_as(
            "select count(1) from history
            where user_id = $1
            and timestamp >= $2::date
            and timestamp < $3::date",
        )
        .bind(user.id)
        .bind(start)
        .bind(end)
        .fetch_one(&self.pool)
        .await?;

        Ok(res.0)
    }

    // Count the history for a given year
    #[instrument(skip_all)]
    async fn count_history_year(&self, user: &User, year: i32) -> Result<i64> {
        let start = chrono::Utc
            .with_ymd_and_hms(year, 1, 1, 0, 0, 0)
            .single()
            .expect("invalid year");
        let end = start + RelativeDuration::years(1);

        let res = self
            .count_history_range(user, start.naive_utc(), end.naive_utc())
            .await?;
        Ok(res)
    }

    // Count the history for a given month
    #[instrument(skip_all)]
    async fn count_history_month(&self, user: &User, month: chrono::NaiveDate) -> Result<i64> {
        let start = month
            .with_day(1)
            .unwrap() // should be unconditionally safe
            .and_time(chrono::NaiveTime::from_hms_opt(0, 0, 0).unwrap()); // should be unconditionally safe also
        let end = start.checked_add_months(chrono::Months::new(1)).unwrap(); // None in the case of overflow, which is kinda impossible here

        debug!("start: {}, end: {}", start, end);

        let res = self.count_history_range(user, start, end).await?;
        Ok(res)
    }

    // Count the history for a given day
    #[instrument(skip_all)]
    async fn count_history_day(&self, user: &User, day: chrono::NaiveDate) -> Result<i64> {
        let start = day.and_time(chrono::NaiveTime::from_hms_opt(0, 0, 0).unwrap());
        let end = start.checked_add_days(chrono::Days::new(1)).unwrap();

        let res = self.count_history_range(user, start, end).await?;
        Ok(res)
    }

    #[instrument(skip_all)]
    async fn list_history(
        &self,
        user: &User,
        created_after: chrono::NaiveDateTime,
        since: chrono::NaiveDateTime,
        host: &str,
    ) -> Result<Vec<History>> {
        let res = sqlx::query_as::<_, History>(
            "select id, client_id, user_id, hostname, timestamp, data, created_at from history
            where user_id = $1
            and hostname != $2
            and created_at >= $3
            and timestamp >= $4
            order by timestamp asc
            limit $5",
        )
        .bind(user.id)
        .bind(host)
        .bind(created_after)
        .bind(since)
        .bind(HISTORY_PAGE_SIZE)
        .fetch_all(&self.pool)
        .await?;

        Ok(res)
    }

    #[instrument(skip_all)]
    async fn add_history(&self, history: &[NewHistory]) -> Result<()> {
        let mut tx = self.pool.begin().await?;

        for i in history {
            let client_id: &str = &i.client_id;
            let hostname: &str = &i.hostname;
            let data: &str = &i.data;

            if data.len() > self.settings.max_history_length
                && self.settings.max_history_length != 0
            {
                // Don't return an error here. We want to insert as much of the
                // history list as we can, so log the error and continue going.

                warn!(
                    "history too long, got length {}, max {}",
                    data.len(),
                    self.settings.max_history_length
                );

                continue;
            }

            sqlx::query(
                "insert into history
                    (client_id, user_id, hostname, timestamp, data)
                values ($1, $2, $3, $4, $5)
                on conflict do nothing
                ",
            )
            .bind(client_id)
            .bind(i.user_id)
            .bind(hostname)
            .bind(i.timestamp)
            .bind(data)
            .execute(&mut tx)
            .await?;
        }

        tx.commit().await?;

        Ok(())
    }

    #[instrument(skip_all)]
    async fn add_user(&self, user: &NewUser) -> Result<i64> {
        let email: &str = &user.email;
        let username: &str = &user.username;
        let password: &str = &user.password;

        let res: (i64,) = sqlx::query_as(
            "insert into users
                (username, email, password)
            values($1, $2, $3)
            returning id",
        )
        .bind(username)
        .bind(email)
        .bind(password)
        .fetch_one(&self.pool)
        .await?;

        Ok(res.0)
    }

    #[instrument(skip_all)]
    async fn add_session(&self, session: &NewSession) -> Result<()> {
        let token: &str = &session.token;

        sqlx::query(
            "insert into sessions
                (user_id, token)
            values($1, $2)",
        )
        .bind(session.user_id)
        .bind(token)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    #[instrument(skip_all)]
    async fn get_user_session(&self, u: &User) -> Result<Session> {
        sqlx::query_as::<_, Session>("select id, user_id, token from sessions where user_id = $1")
            .bind(u.id)
            .fetch_one(&self.pool)
            .await
    }

    #[instrument(skip_all)]
    async fn oldest_history(&self, user: &User) -> Result<History> {
        let res = sqlx::query_as::<_, History>(
            "select id, client_id, user_id, hostname, timestamp, data, created_at from history
            where user_id = $1
            order by timestamp asc
            limit 1",
        )
        .bind(user.id)
        .fetch_one(&self.pool)
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
    ) -> Result<HashMap<u64, TimePeriodInfo>> {
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
                            chrono::NaiveDate::from_ymd_opt(year as i32, month as u32, 1).unwrap(),
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
                            chrono::NaiveDate::from_ymd_opt(year as i32, month as u32, day as u32)
                                .unwrap(),
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
