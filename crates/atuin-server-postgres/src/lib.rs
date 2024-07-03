use std::collections::HashMap;
use std::fmt::Debug;
use std::ops::Range;

use async_trait::async_trait;
use atuin_common::record::{EncryptedData, HostId, Record, RecordIdx, RecordStatus};
use atuin_common::utils::crypto_random_string;
use atuin_server_database::models::{History, NewHistory, NewSession, NewUser, Session, User};
use atuin_server_database::{Database, DbError, DbResult};
use futures_util::TryStreamExt;
use metrics::counter;
use serde::{Deserialize, Serialize};
use sqlx::postgres::PgPoolOptions;
use sqlx::Row;

use time::{OffsetDateTime, PrimitiveDateTime, UtcOffset};
use tracing::{instrument, trace};
use uuid::Uuid;
use wrappers::{DbHistory, DbRecord, DbSession, DbUser};

mod wrappers;

const MIN_PG_VERSION: u32 = 14;

#[derive(Clone)]
pub struct Postgres {
    pool: sqlx::Pool<sqlx::postgres::Postgres>,
}

#[derive(Clone, Deserialize, Serialize)]
pub struct PostgresSettings {
    pub db_uri: String,
}

// Do our best to redact passwords so they're not logged in the event of an error.
impl Debug for PostgresSettings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let redacted_uri = url::Url::parse(&self.db_uri)
            .map(|mut url| {
                let _ = url.set_password(Some("****"));
                url.to_string()
            })
            .unwrap_or(self.db_uri.clone());
        f.debug_struct("PostgresSettings")
            .field("db_uri", &redacted_uri)
            .finish()
    }
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

        // Call server_version_num to get the DB server's major version number
        // The call returns None for servers older than 8.x.
        let pg_major_version: u32 = pool
            .acquire()
            .await
            .map_err(fix_error)?
            .server_version_num()
            .ok_or(DbError::Other(eyre::Report::msg(
                "could not get PostgreSQL version",
            )))?
            / 10000;

        if pg_major_version < MIN_PG_VERSION {
            return Err(DbError::Other(eyre::Report::msg(format!(
                "unsupported PostgreSQL version {}, minimum required is {}",
                pg_major_version, MIN_PG_VERSION
            ))));
        }

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
        sqlx::query_as(
            "select id, username, email, password, verified_at from users where username = $1",
        )
        .bind(username)
        .fetch_one(&self.pool)
        .await
        .map_err(fix_error)
        .map(|DbUser(user)| user)
    }

    #[instrument(skip_all)]
    async fn user_verified(&self, id: i64) -> DbResult<bool> {
        let res: (bool,) =
            sqlx::query_as("select verified_at is not null from users where id = $1")
                .bind(id)
                .fetch_one(&self.pool)
                .await
                .map_err(fix_error)?;

        Ok(res.0)
    }

    #[instrument(skip_all)]
    async fn verify_user(&self, id: i64) -> DbResult<()> {
        sqlx::query(
            "update users set verified_at = (current_timestamp at time zone 'utc') where id=$1",
        )
        .bind(id)
        .execute(&self.pool)
        .await
        .map_err(fix_error)?;

        Ok(())
    }

    /// Return a valid verification token for the user
    /// If the user does not have any token, create one, insert it, and return
    /// If the user has a token, but it's invalid, delete it, create a new one, return
    /// If the user already has a valid token, return it
    #[instrument(skip_all)]
    async fn user_verification_token(&self, id: i64) -> DbResult<String> {
        const TOKEN_VALID_MINUTES: i64 = 15;

        // First we check if there is a verification token
        let token: Option<(String, sqlx::types::time::OffsetDateTime)> = sqlx::query_as(
            "select token, valid_until from user_verification_token where user_id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(fix_error)?;

        let token = if let Some((token, valid_until)) = token {
            trace!("Token for user {id} valid until {valid_until}");

            // We have a token, AND it's still valid
            if valid_until > time::OffsetDateTime::now_utc() {
                token
            } else {
                // token has expired. generate a new one, return it
                let token = crypto_random_string::<24>();

                sqlx::query("update user_verification_token set token = $2, valid_until = $3 where user_id=$1")
                    .bind(id)
                    .bind(&token)
                    .bind(time::OffsetDateTime::now_utc() + time::Duration::minutes(TOKEN_VALID_MINUTES))
                    .execute(&self.pool)
                    .await
                    .map_err(fix_error)?;

                token
            }
        } else {
            // No token in the database! Generate one, insert it
            let token = crypto_random_string::<24>();

            sqlx::query("insert into user_verification_token (user_id, token, valid_until) values ($1, $2, $3)")
                .bind(id)
                .bind(&token)
                .bind(time::OffsetDateTime::now_utc() + time::Duration::minutes(TOKEN_VALID_MINUTES))
                .execute(&self.pool)
                .await
                .map_err(fix_error)?;

            token
        };

        Ok(token)
    }

    #[instrument(skip_all)]
    async fn get_session_user(&self, token: &str) -> DbResult<User> {
        sqlx::query_as(
            "select users.id, users.username, users.email, users.password, users.verified_at from users 
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
    async fn total_history(&self) -> DbResult<i64> {
        // The cache is new, and the user might not yet have a cache value.
        // They will have one as soon as they post up some new history, but handle that
        // edge case.

        let res: (i64,) = sqlx::query_as("select sum(total) from total_history_count_user")
            .fetch_optional(&self.pool)
            .await
            .map_err(fix_error)?
            .unwrap_or((0,));

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

    async fn delete_store(&self, user: &User) -> DbResult<()> {
        sqlx::query(
            "delete from store
            where user_id = $1",
        )
        .bind(user.id)
        .execute(&self.pool)
        .await
        .map_err(fix_error)?;

        Ok(())
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
        .bind(OffsetDateTime::now_utc())
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
        range: Range<OffsetDateTime>,
    ) -> DbResult<i64> {
        let res: (i64,) = sqlx::query_as(
            "select count(1) from history
            where user_id = $1
            and timestamp >= $2::date
            and timestamp < $3::date",
        )
        .bind(user.id)
        .bind(into_utc(range.start))
        .bind(into_utc(range.end))
        .fetch_one(&self.pool)
        .await
        .map_err(fix_error)?;

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
            where user_id = $1
            and hostname != $2
            and created_at >= $3
            and timestamp >= $4
            order by timestamp asc
            limit $5",
        )
        .bind(user.id)
        .bind(host)
        .bind(into_utc(created_after))
        .bind(into_utc(since))
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
            .execute(&mut *tx)
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

        sqlx::query("delete from history where user_id = $1")
            .bind(u.id)
            .execute(&self.pool)
            .await
            .map_err(fix_error)?;

        sqlx::query("delete from store where user_id = $1")
            .bind(u.id)
            .execute(&self.pool)
            .await
            .map_err(fix_error)?;

        sqlx::query("delete from user_verification_token where user_id = $1")
            .bind(u.id)
            .execute(&self.pool)
            .await
            .map_err(fix_error)?;

        sqlx::query("delete from total_history_count_user where user_id = $1")
            .bind(u.id)
            .execute(&self.pool)
            .await
            .map_err(fix_error)?;

        sqlx::query("delete from users where id = $1")
            .bind(u.id)
            .execute(&self.pool)
            .await
            .map_err(fix_error)?;

        Ok(())
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

    #[instrument(skip_all)]
    async fn add_records(&self, user: &User, records: &[Record<EncryptedData>]) -> DbResult<()> {
        let mut tx = self.pool.begin().await.map_err(fix_error)?;

        // We won't have uploaded this data if it wasn't the max. Therefore, we can deduce the max
        // idx without having to make further database queries. Doing the query on this small
        // amount of data should be much, much faster.
        //
        // Worst case, say we get this wrong. We end up caching data that isn't actually the max
        // idx, so clients upload again. The cache logic can be verified with a sql query anyway :)

        let mut heads = HashMap::<(HostId, &str), u64>::new();

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
            .await
            .map_err(fix_error)?;

            // we're already iterating sooooo
            heads
                .entry((i.host.id, &i.tag))
                .and_modify(|e| {
                    if i.idx > *e {
                        *e = i.idx
                    }
                })
                .or_insert(i.idx);
        }

        // we've built the map of heads for this push, so commit it to the database
        for ((host, tag), idx) in heads {
            sqlx::query(
                "insert into store_idx_cache
                    (user_id, host, tag, idx) 
                values ($1, $2, $3, $4)
                on conflict(user_id, host, tag) do update set idx = greatest(store_idx_cache.idx, $4)
                ",
            )
            .bind(user.id)
            .bind(host)
            .bind(tag)
            .bind(idx as i64)
            .execute(&mut *tx)
            .await
            .map_err(fix_error)?;
        }

        tx.commit().await.map_err(fix_error)?;

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
        .map_err(fix_error);

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

        let mut res: Vec<(Uuid, String, i64)> = sqlx::query_as(STATUS_SQL)
            .bind(user.id)
            .fetch_all(&self.pool)
            .await
            .map_err(fix_error)?;
        res.sort();

        // We're temporarily increasing latency in order to improve confidence in the cache
        // If it runs for a few days, and we confirm that cached values are equal to realtime, we
        // can replace realtime with cached.
        //
        // But let's check so sync doesn't do Weird Things.

        let mut cached_res: Vec<(Uuid, String, i64)> =
            sqlx::query_as("select host, tag, idx from store_idx_cache where user_id = $1")
                .bind(user.id)
                .fetch_all(&self.pool)
                .await
                .map_err(fix_error)?;
        cached_res.sort();

        let mut status = RecordStatus::new();

        let equal = res == cached_res;

        if equal {
            counter!("atuin_store_idx_cache_consistent", 1);
        } else {
            // log the values if we have an inconsistent cache
            tracing::debug!(user = user.username, cache_match = equal, res = ?res, cached = ?cached_res, "record store index request");
            counter!("atuin_store_idx_cache_inconsistent", 1);
        };

        for i in res.iter() {
            status.set_raw(HostId(i.0), i.1.clone(), i.2 as u64);
        }

        Ok(status)
    }
}

fn into_utc(x: OffsetDateTime) -> PrimitiveDateTime {
    let x = x.to_offset(UtcOffset::UTC);
    PrimitiveDateTime::new(x.date(), x.time())
}

#[cfg(test)]
mod tests {
    use time::macros::datetime;

    use crate::into_utc;

    #[test]
    fn utc() {
        let dt = datetime!(2023-09-26 15:11:02 +05:30);
        assert_eq!(into_utc(dt), datetime!(2023-09-26 09:41:02));
        assert_eq!(into_utc(dt).assume_utc(), dt);

        let dt = datetime!(2023-09-26 15:11:02 -07:00);
        assert_eq!(into_utc(dt), datetime!(2023-09-26 22:11:02));
        assert_eq!(into_utc(dt).assume_utc(), dt);

        let dt = datetime!(2023-09-26 15:11:02 +00:00);
        assert_eq!(into_utc(dt), datetime!(2023-09-26 15:11:02));
        assert_eq!(into_utc(dt).assume_utc(), dt);
    }
}
