mod wrappers;

use async_trait::async_trait;
use atuin_common::db;
use atuin_domain::record::{
    EncryptedData, HostId, Record, RecordIdx, RecordSeriesKey, RecordStatus, RecordTag,
};
use atuin_server_database::models::{NewSession, NewUser, Session, User};
use atuin_server_database::{Database, DbError, DbResult, DbSettings};
use sqlx::mysql::MySqlPoolOptions;
use tracing::instrument;
use uuid::Uuid;
use wrappers::DbRecord;

#[derive(Clone)]
pub struct MySql {
    pool: sqlx::Pool<sqlx::mysql::MySql>,
    /// Optional read replica pool for read-only queries
    read_pool: Option<sqlx::Pool<sqlx::mysql::MySql>>,
}

impl MySql {
    /// Returns the appropriate pool for read operations.
    /// Uses read_pool if available, otherwise falls back to the primary pool.
    fn read_pool(&self) -> &sqlx::Pool<sqlx::mysql::MySql> {
        self.read_pool.as_ref().unwrap_or(&self.pool)
    }
}

#[async_trait]
impl Database for MySql {
    async fn new(settings: &DbSettings) -> DbResult<Self> {
        let pool =
            MySqlPoolOptions::new().max_connections(100).connect_lazy(settings.db_uri.as_str())?;

        db::migrate!(&pool, "./migrations").await.map_err(|error| DbError::Other(error.into()))?;

        let read_pool = if let Some(read_db_uri) = &settings.read_db_uri {
            tracing::info!("Connecting to read replica database");
            let read_pool =
                MySqlPoolOptions::new().max_connections(100).connect(read_db_uri.as_str()).await?;

            Some(read_pool)
        } else {
            None
        };
        Ok(Self { pool, read_pool })
    }

    #[instrument(skip_all)]
    async fn get_session(&self, token: &str) -> DbResult<Session> {
        db::query_as("select id, user_id, token from sessions where token = ?")
            .bind(token)
            .fetch_one(self.read_pool())
            .await
            .map_err(Into::into)
    }

    #[instrument(skip_all)]
    async fn get_session_user(&self, token: &str) -> DbResult<User> {
        db::query_as(
            "select users.id, users.username, users.email, users.password from users
            inner join sessions
            on users.id = sessions.user_id
            and sessions.token = ?",
        )
        .bind(token)
        .fetch_one(self.read_pool())
        .await
        .map_err(Into::into)
    }

    #[instrument(skip_all)]
    async fn add_session(&self, session: &NewSession) -> DbResult<()> {
        let token: &str = &session.token;

        db::query(
            "insert into sessions
                (user_id, token)
            values(?, ?)",
        )
        .bind(session.user_id)
        .bind(token)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    #[instrument(skip_all)]
    async fn get_user(&self, username: &str) -> DbResult<User> {
        db::query_as("select id, username, email, password from users where username = ?")
            .bind(username)
            .fetch_one(self.read_pool())
            .await
            .map_err(Into::into)
    }

    #[instrument(skip_all)]
    async fn get_user_session(&self, u: &User) -> DbResult<Session> {
        db::query_as("select id, user_id, token from sessions where user_id = ?")
            .bind(u.id)
            .fetch_one(self.read_pool())
            .await
            .map_err(Into::into)
    }

    #[instrument(skip_all)]
    async fn add_user(&self, user: &NewUser) -> DbResult<i64> {
        let email: &str = &user.email;
        let username: &str = &user.username;
        let password: &str = &user.password;

        let res = db::query(
            "insert into users
                (username, email, password)
            values(?, ?, ?)",
        )
        .bind(username)
        .bind(email)
        .bind(password)
        .execute(&self.pool)
        .await?;

        Ok(res.last_insert_id() as i64)
    }

    #[instrument(skip_all)]
    async fn update_user_password(&self, u: &User) -> DbResult<()> {
        db::query(
            "update users
            set password = ?
            where id = ?",
        )
        .bind(&u.password)
        .bind(u.id)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    #[instrument(skip_all)]
    async fn delete_user(&self, u: &User) -> DbResult<()> {
        db::query("delete from sessions where user_id = ?").bind(u.id).execute(&self.pool).await?;

        db::query("delete from history where user_id = ?").bind(u.id).execute(&self.pool).await?;

        db::query("delete from store where user_id = ?").bind(u.id).execute(&self.pool).await?;

        db::query("delete from users where id = ?").bind(u.id).execute(&self.pool).await?;

        Ok(())
    }

    #[instrument(skip_all)]
    async fn delete_store(&self, user: &User) -> DbResult<()> {
        db::query("delete from store where user_id = ?").bind(user.id).execute(&self.pool).await?;

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
                values (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                on duplicate key update id = id
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

        let records: Result<Vec<DbRecord>, DbError> = db::query_as(
            "select client_id, host, idx, timestamp, version, tag, data, cek from store
                    where user_id = ?
                    and tag = ?
                    and host = ?
                    and idx >= ?
                    order by idx asc
                    limit ?",
        )
        .bind(user.id)
        .bind(series.tag.as_str())
        .bind(series.host_id)
        .bind(start as i64)
        .bind(count as i64)
        .fetch_all(self.read_pool())
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
                tracing::debug!("no records found in store: {:?}/{}", series.host_id, series.tag);
                return Ok(vec![]);
            }
            Err(e) => return Err(e),
        };

        Ok(ret)
    }

    #[instrument(skip_all)]
    async fn status(&self, user: &User) -> DbResult<RecordStatus> {
        const STATUS_SQL: &str =
            "select host, tag, max(idx) from store where user_id = ? group by host, tag";

        let mut res: Vec<(Vec<u8>, String, i64)> =
            db::query_as(STATUS_SQL).bind(user.id).fetch_all(self.read_pool()).await?;

        res.sort();

        let mut status = RecordStatus::new();

        for i in &res {
            let host_uuid = Uuid::from_slice(&i.0).map_err(|e| DbError::Other(e.into()))?;
            status.set_raw(
                RecordSeriesKey::new(HostId(host_uuid), RecordTag::from(i.1.clone())),
                i.2 as u64,
            );
        }

        Ok(status)
    }
}
