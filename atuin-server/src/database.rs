use async_trait::async_trait;

use eyre::{eyre, Result};
use sqlx::postgres::PgPoolOptions;

use crate::settings::HISTORY_PAGE_SIZE;

use super::models::{History, NewHistory, NewSession, NewUser, Session, User};

#[async_trait]
pub trait Database {
    async fn get_session(&self, token: &str) -> Result<Session>;
    async fn get_session_user(&self, token: &str) -> Result<User>;
    async fn add_session(&self, session: &NewSession) -> Result<()>;

    async fn get_user(&self, username: String) -> Result<User>;
    async fn get_user_session(&self, u: &User) -> Result<Session>;
    async fn add_user(&self, user: NewUser) -> Result<i64>;

    async fn count_history(&self, user: &User) -> Result<i64>;
    async fn list_history(
        &self,
        user: &User,
        created_since: chrono::NaiveDateTime,
        since: chrono::NaiveDateTime,
        host: String,
    ) -> Result<Vec<History>>;
    async fn add_history(&self, history: &[NewHistory]) -> Result<()>;
}

#[derive(Clone)]
pub struct Postgres {
    pool: sqlx::Pool<sqlx::postgres::Postgres>,
}

impl Postgres {
    pub async fn new(uri: &str) -> Result<Self, sqlx::Error> {
        let pool = PgPoolOptions::new()
            .max_connections(100)
            .connect(uri)
            .await?;

        sqlx::migrate!("./migrations").run(&pool).await?;

        Ok(Self { pool })
    }
}

#[async_trait]
impl Database for Postgres {
    async fn get_session(&self, token: &str) -> Result<Session> {
        let res: Option<Session> =
            sqlx::query_as::<_, Session>("select * from sessions where token = $1")
                .bind(token)
                .fetch_optional(&self.pool)
                .await?;

        if let Some(s) = res {
            Ok(s)
        } else {
            Err(eyre!("could not find session"))
        }
    }

    async fn get_user(&self, username: String) -> Result<User> {
        let res: Option<User> =
            sqlx::query_as::<_, User>("select * from users where username = $1")
                .bind(username)
                .fetch_optional(&self.pool)
                .await?;

        if let Some(u) = res {
            Ok(u)
        } else {
            Err(eyre!("could not find user"))
        }
    }

    async fn get_session_user(&self, token: &str) -> Result<User> {
        let res: Option<User> = sqlx::query_as::<_, User>(
            "select * from users 
            inner join sessions 
            on users.id = sessions.user_id 
            and sessions.token = $1",
        )
        .bind(token)
        .fetch_optional(&self.pool)
        .await?;

        if let Some(u) = res {
            Ok(u)
        } else {
            Err(eyre!("could not find user"))
        }
    }

    async fn count_history(&self, user: &User) -> Result<i64> {
        let res: (i64,) = sqlx::query_as(
            "select count(1) from history
            where user_id = $1",
        )
        .bind(user.id)
        .fetch_one(&self.pool)
        .await?;

        Ok(res.0)
    }

    async fn list_history(
        &self,
        user: &User,
        created_since: chrono::NaiveDateTime,
        since: chrono::NaiveDateTime,
        host: String,
    ) -> Result<Vec<History>> {
        let res = sqlx::query_as::<_, History>(
            "select * from history 
            where user_id = $1
            and hostname != $2
            and created_at >= $3
            and timestamp >= $4
            order by timestamp asc
            limit $5",
        )
        .bind(user.id)
        .bind(host)
        .bind(created_since)
        .bind(since)
        .bind(HISTORY_PAGE_SIZE)
        .fetch_all(&self.pool)
        .await?;

        Ok(res)
    }

    async fn add_history(&self, history: &[NewHistory]) -> Result<()> {
        let mut tx = self.pool.begin().await?;

        for i in history {
            sqlx::query(
                "insert into history
                    (client_id, user_id, hostname, timestamp, data) 
                values ($1, $2, $3, $4, $5)
                on conflict do nothing
                ",
            )
            .bind(i.client_id)
            .bind(i.user_id)
            .bind(i.hostname)
            .bind(i.timestamp)
            .bind(i.data)
            .execute(&mut tx)
            .await?;
        }

        tx.commit().await?;

        Ok(())
    }

    async fn add_user(&self, user: NewUser) -> Result<i64> {
        let res: (i64,) = sqlx::query_as(
            "insert into users
                (username, email, password)
            values($1, $2, $3)
            returning id",
        )
        .bind(user.username.as_str())
        .bind(user.email.as_str())
        .bind(user.password)
        .fetch_one(&self.pool)
        .await?;

        Ok(res.0)
    }

    async fn add_session(&self, session: &NewSession) -> Result<()> {
        sqlx::query(
            "insert into sessions
                (user_id, token)
            values($1, $2)",
        )
        .bind(session.user_id)
        .bind(session.token)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn get_user_session(&self, u: &User) -> Result<Session> {
        let res: Option<Session> =
            sqlx::query_as::<_, Session>("select * from sessions where user_id = $1")
                .bind(u.id)
                .fetch_optional(&self.pool)
                .await?;

        if let Some(s) = res {
            Ok(s)
        } else {
            Err(eyre!("could not find session"))
        }
    }
}
