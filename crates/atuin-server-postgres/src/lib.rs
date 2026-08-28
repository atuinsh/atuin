use async_trait::async_trait;
use atuin_common::db;
use atuin_domain::record::{
    EncryptedData, HostId, Record, RecordIdx, RecordSeriesKey, RecordStatus, RecordTag,
};
use atuin_server_database::models::{NewSession, NewUser, Session, User};
use atuin_server_database::{Database, DbError, DbResult, DbSettings};
use sqlx::postgres::PgPoolOptions;
use tracing::instrument;
use uuid::Uuid;
use wrappers::DbRecord;

mod wrappers;

const MIN_PG_VERSION: u32 = 14;

#[derive(Clone)]
pub struct Postgres {
    pool: sqlx::Pool<sqlx::postgres::Postgres>,
    /// Optional read replica pool for read-only queries
    read_pool: Option<sqlx::Pool<sqlx::postgres::Postgres>>,
}

impl Postgres {
    /// Returns the appropriate pool for read operations.
    /// Uses read_pool if available, otherwise falls back to the primary pool.
    fn read_pool(&self) -> &sqlx::Pool<sqlx::postgres::Postgres> {
        self.read_pool.as_ref().unwrap_or(&self.pool)
    }
}

#[async_trait]
impl Database for Postgres {
    async fn new(settings: &DbSettings) -> DbResult<Self> {
        let pool =
            PgPoolOptions::new().max_connections(100).connect(settings.db_uri.as_str()).await?;

        // Call server_version_num to get the DB server's major version number
        // The call returns None for servers older than 8.x.
        let pg_major_version: u32 = pool
            .acquire()
            .await?
            .server_version_num()
            .ok_or(DbError::Other(eyre::Report::msg("could not get PostgreSQL version")))?
            / 10000;

        if pg_major_version < MIN_PG_VERSION {
            return Err(DbError::Other(eyre::Report::msg(format!(
                "unsupported PostgreSQL version {pg_major_version}, minimum required is \
                 {MIN_PG_VERSION}"
            ))));
        }

        db::migrate!(&pool, "./migrations").await.map_err(|error| DbError::Other(error.into()))?;

        // Create read replica pool if configured
        let read_pool = if let Some(read_db_uri) = &settings.read_db_uri {
            tracing::info!("Connecting to read replica database");
            let read_pool =
                PgPoolOptions::new().max_connections(100).connect(read_db_uri.as_str()).await?;

            // Verify the read replica is also a supported PostgreSQL version
            let read_pg_major_version: u32 =
                read_pool.acquire().await?.server_version_num().ok_or(DbError::Other(
                    eyre::Report::msg("could not get PostgreSQL version from read replica"),
                ))? / 10000;

            if read_pg_major_version < MIN_PG_VERSION {
                return Err(DbError::Other(eyre::Report::msg(format!(
                    "unsupported PostgreSQL version {read_pg_major_version} on read replica, \
                     minimum required is {MIN_PG_VERSION}"
                ))));
            }

            Some(read_pool)
        } else {
            None
        };

        Ok(Self { pool, read_pool })
    }

    #[instrument(skip_all)]
    async fn get_session(&self, token: &str) -> DbResult<Session> {
        db::query_as("select id, user_id, token from sessions where token = $1")
            .bind(token)
            .fetch_one(self.read_pool())
            .await
            .map_err(Into::into)
    }

    #[instrument(skip_all)]
    async fn get_user(&self, username: &str) -> DbResult<User> {
        db::query_as("select id, username, email, password from users where username = $1")
            .bind(username)
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
            and sessions.token = $1",
        )
        .bind(token)
        .fetch_one(self.read_pool())
        .await
        .map_err(Into::into)
    }

    async fn delete_store(&self, user: &User) -> DbResult<()> {
        db::query("delete from store where user_id = $1").bind(user.id).execute(&self.pool).await?;

        Ok(())
    }

    #[instrument(skip_all)]
    async fn delete_user(&self, u: &User) -> DbResult<()> {
        db::query("delete from sessions where user_id = $1").bind(u.id).execute(&self.pool).await?;

        db::query("delete from history where user_id = $1").bind(u.id).execute(&self.pool).await?;

        db::query("delete from store where user_id = $1").bind(u.id).execute(&self.pool).await?;

        db::query("delete from total_history_count_user where user_id = $1")
            .bind(u.id)
            .execute(&self.pool)
            .await?;

        db::query("delete from users where id = $1").bind(u.id).execute(&self.pool).await?;

        Ok(())
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
    async fn get_user_session(&self, u: &User) -> DbResult<Session> {
        db::query_as("select id, user_id, token from sessions where user_id = $1")
            .bind(u.id)
            .fetch_one(self.read_pool())
            .await
            .map_err(Into::into)
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

        let records: Result<Vec<DbRecord>, DbError> = db::query_as(
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

    async fn status(&self, user: &User) -> DbResult<RecordStatus> {
        const STATUS_SQL: &str =
            "select host, tag, max(idx) from store where user_id = $1 group by host, tag";

        let mut res: Vec<(Uuid, String, i64)> =
            db::query_as(STATUS_SQL).bind(user.id).fetch_all(self.read_pool()).await?;

        res.sort();

        let mut status = RecordStatus::new();

        for i in &res {
            status.set_raw(
                RecordSeriesKey::new(HostId(i.0), RecordTag::from(i.1.clone())),
                i.2 as u64,
            );
        }

        Ok(status)
    }
}
