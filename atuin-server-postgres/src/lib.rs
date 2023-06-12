use async_trait::async_trait;
use atuin_server_database::models::{History, NewHistory, NewSession, NewUser, Session, User};
use atuin_server_database::{Database, DbError, DbResult};
use futures_util::TryStreamExt;
use serde::{Deserialize, Serialize};
use sqlx::postgres::PgPoolOptions;

use sqlx::Row;

use tracing::instrument;
use wrappers::{DbHistory, DbSession, DbUser};

mod wrappers;

#[derive(Clone)]
pub struct Postgres {
    pool: sqlx::Pool<sqlx::postgres::Postgres>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PostgresSettings {
    pub db_uri: String,
}

fn fix_error(error: sqlx::Error) -> DbError {
    match error {
        sqlx::Error::RowNotFound => DbError::NotFound,
        error => DbError::Other(error.into()),
    }
}

#[async_trait]
impl Database for Postgres {
    type Settings = PostgresSettings;
    async fn new(settings: &PostgresSettings) -> DbResult<Self> {
        let pool = PgPoolOptions::new()
            .max_connections(100)
            .connect(settings.db_uri.as_str())
            .await
            .map_err(fix_error)?;

        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .map_err(|error| DbError::Other(error.into()))?;

        Ok(Self { pool })
    }

    #[instrument(skip_all)]
    async fn get_session(&self, token: &str) -> DbResult<Session> {
        sqlx::query_as("select id, user_id, token from sessions where token = $1")
            .bind(token)
            .fetch_one(&self.pool)
            .await
            .map_err(fix_error)
            .map(|DbSession(session)| session)
    }

    #[instrument(skip_all)]
    async fn get_user(&self, username: &str) -> DbResult<User> {
        sqlx::query_as("select id, username, email, password from users where username = $1")
            .bind(username)
            .fetch_one(&self.pool)
            .await
            .map_err(fix_error)
            .map(|DbUser(user)| user)
    }

    #[instrument(skip_all)]
    async fn get_session_user(&self, token: &str) -> DbResult<User> {
        sqlx::query_as(
            "select users.id, users.username, users.email, users.password from users 
            inner join sessions 
            on users.id = sessions.user_id 
            and sessions.token = $1",
        )
        .bind(token)
        .fetch_one(&self.pool)
        .await
        .map_err(fix_error)
        .map(|DbUser(user)| user)
    }

    #[instrument(skip_all)]
    async fn count_history(&self, user: &User) -> DbResult<i64> {
        // The cache is new, and the user might not yet have a cache value.
        // They will have one as soon as they post up some new history, but handle that
        // edge case.

        let res: (i64,) = sqlx::query_as(
            "select count(1) from history
            where user_id = $1",
        )
        .bind(user.id)
        .fetch_one(&self.pool)
        .await
        .map_err(fix_error)?;

        Ok(res.0)
    }

    #[instrument(skip_all)]
    async fn count_history_cached(&self, user: &User) -> DbResult<i64> {
        let res: (i32,) = sqlx::query_as(
            "select total from total_history_count_user
            where user_id = $1",
        )
        .bind(user.id)
        .fetch_one(&self.pool)
        .await
        .map_err(fix_error)?;

        Ok(res.0 as i64)
    }

    async fn delete_history(&self, user: &User, id: String) -> DbResult<()> {
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
        .await
        .map_err(fix_error)?;

        Ok(())
    }

    #[instrument(skip_all)]
    async fn deleted_history(&self, user: &User) -> DbResult<Vec<String>> {
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
        .await
        .map_err(fix_error)?;

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
    ) -> DbResult<i64> {
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
        .await
        .map_err(fix_error)?;

        Ok(res.0)
    }

    #[instrument(skip_all)]
    async fn list_history(
        &self,
        user: &User,
        created_after: chrono::NaiveDateTime,
        since: chrono::NaiveDateTime,
        host: &str,
        page_size: i64,
    ) -> DbResult<Vec<History>> {
        let res = sqlx::query_as(
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
        .bind(page_size)
        .fetch(&self.pool)
        .map_ok(|DbHistory(h)| h)
        .try_collect()
        .await
        .map_err(fix_error)?;

        Ok(res)
    }

    #[instrument(skip_all)]
    async fn add_history(&self, history: &[NewHistory]) -> DbResult<()> {
        let mut tx = self.pool.begin().await.map_err(fix_error)?;

        for i in history {
            let client_id: &str = &i.client_id;
            let hostname: &str = &i.hostname;
            let data: &str = &i.data;

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
            .await
            .map_err(fix_error)?;
        }

        tx.commit().await.map_err(fix_error)?;

        Ok(())
    }

    #[instrument(skip_all)]
    async fn delete_user(&self, u: &User) -> DbResult<()> {
        sqlx::query("delete from sessions where user_id = $1")
            .bind(u.id)
            .execute(&self.pool)
            .await
            .map_err(fix_error)?;

        sqlx::query("delete from users where id = $1")
            .bind(u.id)
            .execute(&self.pool)
            .await
            .map_err(fix_error)?;

        sqlx::query("delete from history where user_id = $1")
            .bind(u.id)
            .execute(&self.pool)
            .await
            .map_err(fix_error)?;

        Ok(())
    }

    #[instrument(skip_all)]
    async fn add_user(&self, user: &NewUser) -> DbResult<i64> {
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
        .await
        .map_err(fix_error)?;

        Ok(res.0)
    }

    #[instrument(skip_all)]
    async fn add_session(&self, session: &NewSession) -> DbResult<()> {
        let token: &str = &session.token;

        sqlx::query(
            "insert into sessions
                (user_id, token)
            values($1, $2)",
        )
        .bind(session.user_id)
        .bind(token)
        .execute(&self.pool)
        .await
        .map_err(fix_error)?;

        Ok(())
    }

    #[instrument(skip_all)]
    async fn get_user_session(&self, u: &User) -> DbResult<Session> {
        sqlx::query_as("select id, user_id, token from sessions where user_id = $1")
            .bind(u.id)
            .fetch_one(&self.pool)
            .await
            .map_err(fix_error)
            .map(|DbSession(session)| session)
    }

    #[instrument(skip_all)]
    async fn oldest_history(&self, user: &User) -> DbResult<History> {
        sqlx::query_as(
            "select id, client_id, user_id, hostname, timestamp, data, created_at from history 
            where user_id = $1
            order by timestamp asc
            limit 1",
        )
        .bind(user.id)
        .fetch_one(&self.pool)
        .await
        .map_err(fix_error)
        .map(|DbHistory(h)| h)
    }
}
