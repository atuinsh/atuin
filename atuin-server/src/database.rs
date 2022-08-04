use std::collections::HashMap;
use std::marker::PhantomData;
use std::ops::Deref;

use async_trait::async_trait;
use sqlx::Result;

use super::{
    calendar::{TimePeriod, TimePeriodInfo},
    models::{History, NewHistory, NewSession, NewUser, Session, User},
};

mod postgres;
mod sqlite;

pub use postgres::Postgres;
pub use sqlite::Sqlite;

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

pub struct DatabaseWrapped<T, DB>
where
    DB: Database,
{
    item: T,
    phantom: PhantomData<DB>,
}

impl<T, DB> Deref for DatabaseWrapped<T, DB>
where
    DB: Database,
{
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.item
    }
}

impl<T, DB> From<T> for DatabaseWrapped<T, DB>
where
    DB: Database,
{
    fn from(item: T) -> Self {
        DatabaseWrapped {
            item,
            phantom: Default::default(),
        }
    }
}
