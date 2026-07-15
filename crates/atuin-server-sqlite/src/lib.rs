use std::str::FromStr;

use async_trait::async_trait;
use atuin_common::record::{EncryptedData, HostId, Record, RecordIdx, RecordStatus};
use atuin_server_database::{
    Database, DbError, DbResult, DbSettings,
    models::{NewSession, NewUser, Session, User},
};
use sqlx::{
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
    types::Uuid,
};
use tracing::instrument;
use wrappers::{DbRecord, DbSession, DbUser};

mod wrappers;

#[derive(Clone)]
pub struct Sqlite {
    pool: sqlx::Pool<sqlx::sqlite::Sqlite>,
}

#[async_trait]
impl Database for Sqlite {
    async fn new(settings: &DbSettings) -> DbResult<Self> {
        let opts = SqliteConnectOptions::from_str(&settings.db_uri)?
            .journal_mode(SqliteJournalMode::Wal)
            .create_if_missing(true);

        let pool = SqlitePoolOptions::new().connect_with(opts).await?;

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
            .map_err(Into::into)
            .map(|DbSession(session)| session)
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
        .map_err(Into::into)
        .map(|DbUser(user)| user)
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
        .await?;

        Ok(())
    }

    #[instrument(skip_all)]
    async fn get_user(&self, username: &str) -> DbResult<User> {
        sqlx::query_as("select id, username, email, password from users where username = $1")
            .bind(username)
            .fetch_one(&self.pool)
            .await
            .map_err(Into::into)
            .map(|DbUser(user)| user)
    }

    #[instrument(skip_all)]
    async fn get_user_session(&self, u: &User) -> DbResult<Session> {
        sqlx::query_as("select id, user_id, token from sessions where user_id = $1")
            .bind(u.id)
            .fetch_one(&self.pool)
            .await
            .map_err(Into::into)
            .map(|DbSession(session)| session)
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
        .await?;

        Ok(res.0)
    }

    #[instrument(skip_all)]
    async fn update_user_password(&self, user: &User) -> DbResult<()> {
        sqlx::query(
            "update users
            set password = $1
            where id = $2",
        )
        .bind(&user.password)
        .bind(user.id)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    #[instrument(skip_all)]
    async fn delete_user(&self, u: &User) -> DbResult<()> {
        sqlx::query("delete from sessions where user_id = $1")
            .bind(u.id)
            .execute(&self.pool)
            .await?;

        sqlx::query("delete from users where id = $1")
            .bind(u.id)
            .execute(&self.pool)
            .await?;

        sqlx::query("delete from history where user_id = $1")
            .bind(u.id)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    async fn delete_store(&self, user: &User) -> DbResult<()> {
        sqlx::query(
            "delete from store
            where user_id = $1",
        )
        .bind(user.id)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    #[instrument(skip_all)]
    async fn add_records(&self, user: &User, records: &[Record<EncryptedData>]) -> DbResult<()> {
        let mut tx = self.pool.begin().await?;

        for i in records {
            let id = atuin_common::utils::uuid_v7();

            sqlx::query(
                "insert into store
                    (id, client_id, host, idx, timestamp, version, tag, data, cek, user_id) 
                values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
                on conflict do nothing
                ",
            )
            .bind(id)
            .bind(i.id)
            .bind(i.host.id)
            .bind(i.idx as i64)
            .bind(i.timestamp as i64) // throwing away some data, but i64 is still big in terms of time
            .bind(&i.version)
            .bind(&i.tag)
            .bind(&i.data.data)
            .bind(&i.data.content_encryption_key)
            .bind(user.id)
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;

        Ok(())
    }

    #[instrument(skip_all)]
    async fn next_records(
        &self,
        user: &User,
        host: HostId,
        tag: String,
        start: Option<RecordIdx>,
        count: u64,
    ) -> DbResult<Vec<Record<EncryptedData>>> {
        tracing::debug!("{:?} - {:?} - {:?}", host, tag, start);
        let start = start.unwrap_or(0);

        let records: Result<Vec<DbRecord>, DbError> = sqlx::query_as(
            "select client_id, host, idx, timestamp, version, tag, data, cek from store
                    where user_id = $1
                    and tag = $2
                    and host = $3
                    and idx >= $4
                    order by idx asc
                    limit $5",
        )
        .bind(user.id)
        .bind(tag.clone())
        .bind(host)
        .bind(start as i64)
        .bind(count as i64)
        .fetch_all(&self.pool)
        .await
        .map_err(Into::into);

        let ret = match records {
            Ok(records) => {
                let records: Vec<Record<EncryptedData>> = records
                    .into_iter()
                    .map(|f| {
                        let record: Record<EncryptedData> = f.into();
                        record
                    })
                    .collect();

                records
            }
            Err(DbError::NotFound) => {
                tracing::debug!("no records found in store: {:?}/{}", host, tag);
                return Ok(vec![]);
            }
            Err(e) => return Err(e),
        };

        Ok(ret)
    }

    async fn status(&self, user: &User) -> DbResult<RecordStatus> {
        const STATUS_SQL: &str =
            "select host, tag, max(idx) from store where user_id = $1 group by host, tag";

        let res: Vec<(Uuid, String, i64)> = sqlx::query_as(STATUS_SQL)
            .bind(user.id)
            .fetch_all(&self.pool)
            .await?;

        let mut status = RecordStatus::new();

        for i in res {
            status.set_raw(HostId(i.0), i.1, i.2 as u64);
        }

        Ok(status)
    }
}
