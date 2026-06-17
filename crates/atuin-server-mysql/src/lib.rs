mod wrappers;

use std::collections::HashMap;
use std::ops::Range;

use async_trait::async_trait;
use atuin_common::record::{EncryptedData, HostId, Record, RecordIdx, RecordStatus};
use atuin_server_database::models::{History, NewHistory, NewSession, NewUser, Session, User};
use atuin_server_database::{Database, DbError, DbResult, DbSettings, into_utc};
use futures_util::TryStreamExt;
use rand::Rng;
use sqlx::Row;
use sqlx::mysql::MySqlPoolOptions;

use time::OffsetDateTime;
use tracing::instrument;
use uuid::Uuid;
use wrappers::{DbHistory, DbRecord, DbSession, DbUser};

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
        let pool = MySqlPoolOptions::new()
            .max_connections(100)
            .connect_lazy(settings.db_uri.as_str())?;

        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .map_err(|error| DbError::Other(error.into()))?;

        let read_pool = if let Some(read_db_uri) = &settings.read_db_uri {
            tracing::info!("Connecting to read replica database");
            let read_pool = MySqlPoolOptions::new()
                .max_connections(100)
                .connect(read_db_uri.as_str())
                .await?;

            Some(read_pool)
        } else {
            None
        };
        Ok(Self { pool, read_pool })
    }

    #[instrument(skip_all)]
    async fn get_session(&self, token: &str) -> DbResult<Session> {
        sqlx::query_as("select id, user_id, token from sessions where token = ?")
            .bind(token)
            .fetch_one(self.read_pool())
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
            and sessions.token = ?",
        )
        .bind(token)
        .fetch_one(self.read_pool())
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
        sqlx::query_as("select id, username, email, password from users where username = ?")
            .bind(username)
            .fetch_one(self.read_pool())
            .await
            .map_err(Into::into)
            .map(|DbUser(user)| user)
    }

    #[instrument(skip_all)]
    async fn get_user_session(&self, u: &User) -> DbResult<Session> {
        sqlx::query_as("select id, user_id, token from sessions where user_id = ?")
            .bind(u.id)
            .fetch_one(self.read_pool())
            .await
            .map_err(Into::into)
            .map(|DbSession(session)| session)
    }

    #[instrument(skip_all)]
    async fn add_user(&self, user: &NewUser) -> DbResult<i64> {
        let email: &str = &user.email;
        let username: &str = &user.username;
        let password: &str = &user.password;

        let res = sqlx::query(
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
        sqlx::query(
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
    async fn count_history(&self, user: &User) -> DbResult<i64> {
        // The cache is new, and the user might not yet have a cache value.
        // They will have one as soon as they post up some new history, but handle that
        // edge case.

        let res: (i64,) = sqlx::query_as(
            "select count(1) from history
            where user_id = ?",
        )
        .bind(user.id)
        .fetch_one(self.read_pool())
        .await?;

        Ok(res.0)
    }

    #[instrument(skip_all)]
    async fn count_history_cached(&self, _user: &User) -> DbResult<i64> {
        Err(DbError::NotFound)
    }

    #[instrument(skip_all)]
    async fn delete_user(&self, u: &User) -> DbResult<()> {
        sqlx::query("delete from sessions where user_id = ?")
            .bind(u.id)
            .execute(&self.pool)
            .await?;

        sqlx::query("delete from history where user_id = ?")
            .bind(u.id)
            .execute(&self.pool)
            .await?;

        sqlx::query("delete from store where user_id = ?")
            .bind(u.id)
            .execute(&self.pool)
            .await?;

        sqlx::query("delete from users where id = ?")
            .bind(u.id)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    #[instrument(skip_all)]
    async fn delete_history(&self, user: &User, id: String) -> DbResult<()> {
        sqlx::query(
            "update history
            set deleted_at = ?
            where user_id = ?
            and client_id = ?
            and deleted_at is null", // don't just keep setting it
        )
        .bind(OffsetDateTime::now_utc())
        .bind(user.id)
        .bind(id)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    #[instrument(skip_all)]
    async fn deleted_history(&self, user: &User) -> DbResult<Vec<String>> {
        // The cache is new, and the user might not yet have a cache value.
        // They will have one as soon as they post up some new history, but handle that
        // edge case.

        let res = sqlx::query(
            "select client_id from history
            where user_id = ?
            and deleted_at is not null",
        )
        .bind(user.id)
        .fetch_all(self.read_pool())
        .await?;

        let res = res
            .iter()
            .map(|row| row.get::<String, _>("client_id"))
            .collect();

        Ok(res)
    }

    #[instrument(skip_all)]
    async fn delete_store(&self, user: &User) -> DbResult<()> {
        let mut tx = self.pool.begin().await?;

        sqlx::query(
            "delete from store
            where user_id = ?",
        )
        .bind(user.id)
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            "delete from store_idx_cache
            where user_id = ?",
        )
        .bind(user.id)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;

        Ok(())
    }

    #[instrument(skip_all)]
    async fn add_records(&self, user: &User, records: &[Record<EncryptedData>]) -> DbResult<()> {
        let mut tx = self.pool.begin().await?;

        // We won't have uploaded this data if it wasn't the max. Therefore, we can deduce the max
        // idx without having to make further database queries. Doing the query on this small
        // amount of data should be much, much faster.
        //
        // Worst case, say we get this wrong. We end up caching data that isn't actually the max
        // idx, so clients upload again. The cache logic can be verified with a sql query anyway :)

        let mut heads = HashMap::<(HostId, &str), u64>::new();

        for i in records {
            let id = atuin_common::utils::uuid_v7();

            let result = sqlx::query(
                "insert ignore into store
                    (id, client_id, host, idx, timestamp, version, tag, data, cek, user_id) 
                values (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
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

            // Only update heads if we actually inserted the record
            if result.rows_affected() > 0 {
                heads
                    .entry((i.host.id, &i.tag))
                    .and_modify(|e| {
                        if i.idx > *e {
                            *e = i.idx
                        }
                    })
                    .or_insert(i.idx);
            }
        }

        // we've built the map of heads for this push, so commit it to the database
        for ((host, tag), idx) in heads {
            sqlx::query(
                "insert into store_idx_cache
                    (user_id, host, tag, idx) 
                values (?, ?, ?, ?)
                on duplicate key update idx = greatest(idx, ?)
                ",
            )
            .bind(user.id)
            .bind(host)
            .bind(tag)
            .bind(idx as i64)
            .bind(idx as i64)
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
                    where user_id = ?
                    and tag = ?
                    and host = ?
                    and idx >= ?
                    order by idx asc
                    limit ?",
        )
        .bind(user.id)
        .bind(tag.clone())
        .bind(host)
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
                tracing::debug!("no records found in store: {:?}/{}", host, tag);
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

        // If IDX_CACHE_ROLLOUT is set, then we
        // 1. Read the value of the var, use it as a % chance of using the cache
        // 2. If we use the cache, just read from the cache table
        // 3. If we don't use the cache, read from the store table
        // IDX_CACHE_ROLLOUT should be between 0 and 100.

        let idx_cache_rollout = std::env::var("IDX_CACHE_ROLLOUT").unwrap_or("0".to_string());
        let idx_cache_rollout = idx_cache_rollout.parse::<f64>().unwrap_or(0.0);
        let use_idx_cache = rand::thread_rng().gen_bool(idx_cache_rollout / 100.0);

        let mut res: Vec<(Vec<u8>, String, i64)> = if use_idx_cache {
            tracing::debug!("using idx cache for user {}", user.id);
            sqlx::query_as("select host, tag, idx from store_idx_cache where user_id = ?")
                .bind(user.id)
                .fetch_all(self.read_pool())
                .await?
        } else {
            tracing::debug!("using aggregate query for user {}", user.id);
            sqlx::query_as(STATUS_SQL)
                .bind(user.id)
                .fetch_all(self.read_pool())
                .await?
        };

        res.sort();

        let mut status = RecordStatus::new();

        for i in res.iter() {
            let host_uuid = Uuid::from_slice(&i.0).map_err(|e| DbError::Other(e.into()))?;
            status.set_raw(HostId(host_uuid), i.1.clone(), i.2 as u64);
        }

        Ok(status)
    }

    #[instrument(skip_all)]
    async fn count_history_range(
        &self,
        user: &User,
        range: Range<OffsetDateTime>,
    ) -> DbResult<i64> {
        let res: (i64,) = sqlx::query_as(
            "select count(1) from history
            where user_id = ?
            and timestamp >= ?
            and timestamp < ?",
        )
        .bind(user.id)
        .bind(into_utc(range.start))
        .bind(into_utc(range.end))
        .fetch_one(self.read_pool())
        .await?;

        Ok(res.0)
    }

    #[instrument(skip_all)]
    async fn list_history(
        &self,
        user: &User,
        created_after: OffsetDateTime,
        since: OffsetDateTime,
        host: &str,
        page_size: i64,
    ) -> DbResult<Vec<History>> {
        let res = sqlx::query_as(
            "select id, client_id, user_id, hostname, timestamp, data, created_at from history
            where user_id = ?
            and hostname != ?
            and created_at >= ?
            and timestamp >= ?
            order by timestamp asc
            limit ?",
        )
        .bind(user.id)
        .bind(host)
        .bind(into_utc(created_after))
        .bind(into_utc(since))
        .bind(page_size)
        .fetch(self.read_pool())
        .map_ok(|DbHistory(h)| h)
        .try_collect()
        .await?;

        Ok(res)
    }

    #[instrument(skip_all)]
    async fn add_history(&self, history: &[NewHistory]) -> DbResult<()> {
        let mut tx = self.pool.begin().await?;

        for i in history {
            let client_id: &str = &i.client_id;
            let hostname: &str = &i.hostname;
            let data: &str = &i.data;

            sqlx::query(
                "insert ignore into history
                    (client_id, user_id, hostname, timestamp, data) 
                values (?, ?, ?, ?, ?)
                ",
            )
            .bind(client_id)
            .bind(i.user_id)
            .bind(hostname)
            .bind(i.timestamp)
            .bind(data)
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;

        Ok(())
    }

    #[instrument(skip_all)]
    async fn oldest_history(&self, user: &User) -> DbResult<History> {
        sqlx::query_as(
            "select id, client_id, user_id, hostname, timestamp, data, created_at from history
            where user_id = ?
            order by timestamp asc
            limit 1",
        )
        .bind(user.id)
        .fetch_one(self.read_pool())
        .await
        .map_err(Into::into)
        .map(|DbHistory(h)| h)
    }
}
