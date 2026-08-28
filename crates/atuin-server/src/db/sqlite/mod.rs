use std::str::FromStr;

use async_trait::async_trait;
use atuin_common::db;
use atuin_common::db::SqliteDbUrl;
use atuin_domain::record::{EncryptedData, Record, RecordIdx, RecordSeriesKey, RecordStatus};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use tracing::instrument;

use super::models::{DbRecord, NewSession, NewUser, RecordSeriesPoint, Session, User};
use super::{Database, DbError, DbResult};

#[derive(Clone)]
pub struct Sqlite {
    pool: sqlx::Pool<sqlx::sqlite::Sqlite>,
}

#[async_trait]
impl Database for Sqlite {
    type Url = SqliteDbUrl;

    async fn connect(url: SqliteDbUrl) -> DbResult<Self> {
        let opts = SqliteConnectOptions::from_str(url.as_str())?
            .journal_mode(SqliteJournalMode::Wal)
            .create_if_missing(true);

        let pool = SqlitePoolOptions::new().connect_with(opts).await?;

        db::migrate!(&pool, "src/db/sqlite/migrations")
            .await
            .map_err(|error| DbError::Other(error.into()))?;

        Ok(Self { pool })
    }

    #[instrument(skip_all)]
    async fn get_session(&self, token: &str) -> DbResult<Session> {
        db::query_as("select id, user_id, token from sessions where token = $1")
            .bind(token)
            .fetch_one(&self.pool)
            .await
            .map_err(Into::into)
    }

    #[instrument(skip_all)]
    async fn get_session_user(&self, token: &str) -> DbResult<User> {
        db::query_as(
            "select users.id, users.username, users.email, users.password from users
            inner join sessions
            on users.id = sessions.user_id
            and sessions.token = $1",
        )
        .bind(token)
        .fetch_one(&self.pool)
        .await
        .map_err(Into::into)
    }

    #[instrument(skip_all)]
    async fn add_session(&self, session: &NewSession) -> DbResult<()> {
        let token: &str = &session.token;

        db::query(
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
        db::query_as("select id, username, email, password from users where username = $1")
            .bind(username)
            .fetch_one(&self.pool)
            .await
            .map_err(Into::into)
    }

    #[instrument(skip_all)]
    async fn get_user_session(&self, u: &User) -> DbResult<Session> {
        db::query_as("select id, user_id, token from sessions where user_id = $1")
            .bind(u.id)
            .fetch_one(&self.pool)
            .await
            .map_err(Into::into)
    }

    #[instrument(skip_all)]
    async fn add_user(&self, user: &NewUser) -> DbResult<i64> {
        let email: &str = &user.email;
        let username: &str = &user.username;
        let password: &str = &user.password;

        let res: (i64,) = db::query_as(
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
        db::query(
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
        db::query("delete from sessions where user_id = $1").bind(u.id).execute(&self.pool).await?;

        db::query("delete from history where user_id = $1").bind(u.id).execute(&self.pool).await?;

        db::query("delete from store where user_id = $1").bind(u.id).execute(&self.pool).await?;

        db::query("delete from users where id = $1").bind(u.id).execute(&self.pool).await?;

        Ok(())
    }

    async fn delete_store(&self, user: &User) -> DbResult<()> {
        db::query(
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

            db::query(
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
            .bind(i.version.as_str())
            .bind(i.tag.as_str())
            .bind(&i.data.raw)
            .bind(&i.data.cek)
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
        series: &RecordSeriesKey,
        start: Option<RecordIdx>,
        count: u64,
    ) -> DbResult<Vec<Record<EncryptedData>>> {
        tracing::debug!("{:?} - {:?} - {:?}", series.host_id, series.tag, start);
        let start = start.unwrap_or(0);

        db::query_as::<_, DbRecord>(
            "select client_id, host, idx, timestamp, version, tag, data, cek from store
                    where user_id = $1
                    and tag = $2
                    and host = $3
                    and idx >= $4
                    order by idx asc
                    limit $5",
        )
        .bind(user.id)
        .bind(series.tag.as_str())
        .bind(series.host_id)
        .bind(start as i64)
        .bind(count as i64)
        .fetch_all(&self.pool)
        .await
        .map(|records| records.into_iter().map(Into::into).collect())
        .map_err(Into::into)
    }

    async fn status(&self, user: &User) -> DbResult<RecordStatus> {
        const STATUS_SQL: &str =
            "select host, tag, max(idx) as idx from store where user_id = $1 group by host, tag";

        let points = db::query_as::<_, RecordSeriesPoint>(STATUS_SQL)
            .bind(user.id)
            .fetch_all(&self.pool)
            .await?;
        Ok(RecordStatus::from_points(points.into_iter().map(Into::into)))
    }
}
