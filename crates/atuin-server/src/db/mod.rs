pub mod entities;
pub mod models;

pub mod postgres;

pub use postgres::Postgres;

pub mod sqlite;

use async_trait::async_trait;
use atuin_common::db::OwnedDbUrl;
use atuin_domain::record::{
    EncryptedData, Host, HostId, Record, RecordId, RecordIdx, RecordSeriesKey, RecordStatus,
    RecordTag, RecordVersion,
};
use sea_orm::ActiveValue::Set;
use sea_orm::sea_query::{Expr, OnConflict};
use sea_orm::{
    ColumnTrait, DatabaseConnection, DbBackend, EntityTrait, FromQueryResult, QueryFilter,
    QueryOrder, QuerySelect, TransactionTrait,
};
use serde::{Deserialize, Serialize};
pub use sqlite::Sqlite;
use tracing::instrument;

pub mod mysql;

pub use mysql::MySql;

use self::models::{NewSession, NewUser, Session, User};

#[derive(Debug, derive_more::Display, derive_more::Error, derive_more::From)]
#[display("{self:?}")]
pub enum DbError {
    #[from(skip)]
    NotFound,
    #[from(time::error::ComponentRange, time::error::Error)]
    Other(eyre::Report),
}

impl From<sqlx::Error> for DbError {
    fn from(error: sqlx::Error) -> Self {
        match error {
            sqlx::Error::RowNotFound => Self::NotFound,
            error => Self::Other(error.into()),
        }
    }
}

impl From<sea_orm::DbErr> for DbError {
    fn from(error: sea_orm::DbErr) -> Self {
        // sea-orm surfaces "no row" as `Option::None` from the query methods we use, so
        // every `DbErr` that reaches here is a genuine failure — not a not-found.
        Self::Other(error.into())
    }
}

pub type DbResult<T> = Result<T, DbError>;

// Password redaction lives on `OwnedDbUrl`'s `Debug`, so the derive is safe here.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DbSettings {
    pub db_uri: OwnedDbUrl,
}

/// Chunk size for `add_records` bulk inserts. 100 rows × 10 columns = 1000 bind
/// parameters, comfortably under SQLite's variable limit and the sync page size.
const ADD_RECORDS_CHUNK: usize = 100;

/// The `status` aggregate — one row of `(host, tag, max(idx))` per series.
#[derive(Debug, FromQueryResult)]
struct StatusRow {
    host: sea_orm::prelude::Uuid,
    tag: String,
    idx: i64,
}

impl From<entities::sessions::Model> for Session {
    fn from(m: entities::sessions::Model) -> Self {
        Self {
            id: m.id,
            user_id: m.user_id,
            token: m.token,
        }
    }
}

impl From<entities::users::Model> for User {
    fn from(m: entities::users::Model) -> Self {
        Self {
            id: m.id,
            username: m.username,
            email: m.email,
            password: m.password,
        }
    }
}

/// The `add_records` upsert, rendered correctly for each dialect.
///
/// The intent is "insert, and on a `record_uniq(user_id, host, tag, idx)` collision keep
/// the existing row unchanged". pg/sqlite express that as `ON CONFLICT (...) DO NOTHING`.
/// MySQL has no `DO NOTHING`; its equivalent is `ON DUPLICATE KEY UPDATE id = id` (a no-op)
/// — which is exactly what the hand-written MySQL backend used to emit.
///
/// We can't feed one sea-query `do_nothing()` to all three: in sea-query 1.0 it renders
/// the invalid `ON DUPLICATE KEY IGNORE` for MySQL. So we branch once, here, and every
/// dialect gets the same SQL the old per-backend code did — pg/sqlite keep the optimal
/// `DO NOTHING` (no row rewrite), MySQL gets its no-op update.
fn record_upsert(backend: DbBackend) -> OnConflict {
    use entities::store::Column;
    let cols = [Column::UserId, Column::Host, Column::Tag, Column::Idx];
    match backend {
        DbBackend::MySql => {
            OnConflict::columns(cols).value(Column::Id, Expr::col(Column::Id)).to_owned()
        }
        // pg, sqlite, and any future dialect: the standard, optimal `DO NOTHING`.
        _ => OnConflict::columns(cols).do_nothing().to_owned(),
    }
}

/// Reconstruct a domain [`Record`] from a `store` row.
fn record_from_model(m: entities::store::Model) -> Record<EncryptedData> {
    Record {
        id: RecordId(m.client_id),
        host: Host::new(HostId(m.host)),
        idx: m.idx as u64,
        timestamp: m.timestamp as u64,
        version: RecordVersion::from(m.version),
        tag: RecordTag::from(m.tag),
        data: EncryptedData {
            raw: m.data,
            cek: m.cek,
        },
    }
}

/// A server database backend.
///
/// The query surface — every method below `conn` — is implemented **once** here as sea-orm
/// default methods, because sea-orm renders dialect-correct SQL from the shared
/// [`entities`]. A backend therefore only has to open its pool ([`connect`](Self::connect))
/// and hand back a sea-orm connection. That is the entire per-backend surface; Postgres,
/// MySQL and SQLite differ only in how they connect.
#[async_trait]
pub trait Database: Sized + Clone + Send + Sync + 'static {
    /// The backend-specific connection URL this database is built from.
    type Url;

    async fn connect(url: Self::Url) -> DbResult<Self>;

    /// The sea-orm connection every query runs against.
    fn conn(&self) -> &DatabaseConnection;

    #[instrument(skip_all)]
    async fn get_session(&self, token: &str) -> DbResult<Session> {
        use entities::sessions;
        sessions::Entity::find()
            .filter(sessions::Column::Token.eq(token))
            .one(self.conn())
            .await?
            .map(Session::from)
            .ok_or(DbError::NotFound)
    }

    #[instrument(skip_all)]
    async fn get_session_user(&self, token: &str) -> DbResult<User> {
        use entities::{sessions, users};
        users::Entity::find()
            .inner_join(sessions::Entity)
            .filter(sessions::Column::Token.eq(token))
            .one(self.conn())
            .await?
            .map(User::from)
            .ok_or(DbError::NotFound)
    }

    #[instrument(skip_all)]
    async fn add_session(&self, session: &NewSession) -> DbResult<()> {
        use entities::sessions;
        let row = sessions::ActiveModel {
            user_id: Set(session.user_id),
            token: Set(session.token.clone()),
            ..Default::default()
        };
        sessions::Entity::insert(row).exec(self.conn()).await?;
        Ok(())
    }

    #[instrument(skip_all)]
    async fn get_user(&self, username: &str) -> DbResult<User> {
        use entities::users;
        users::Entity::find()
            .filter(users::Column::Username.eq(username))
            .one(self.conn())
            .await?
            .map(User::from)
            .ok_or(DbError::NotFound)
    }

    #[instrument(skip_all)]
    async fn get_user_session(&self, u: &User) -> DbResult<Session> {
        use entities::sessions;
        sessions::Entity::find()
            .filter(sessions::Column::UserId.eq(u.id))
            .one(self.conn())
            .await?
            .map(Session::from)
            .ok_or(DbError::NotFound)
    }

    #[instrument(skip_all)]
    async fn add_user(&self, user: &NewUser) -> DbResult<i64> {
        use entities::users;
        let row = users::ActiveModel {
            username: Set(user.username.clone()),
            email: Set(user.email.clone()),
            password: Set(user.password.clone()),
            ..Default::default()
        };
        // sea-orm papers over the one real dialect split here: `RETURNING id` on
        // pg/sqlite vs `last_insert_id()` on mysql.
        let res = users::Entity::insert(row).exec(self.conn()).await?;
        Ok(res.last_insert_id)
    }

    #[instrument(skip_all)]
    async fn update_user_password(&self, u: &User) -> DbResult<()> {
        use entities::users;
        users::Entity::update_many()
            .col_expr(users::Column::Password, Expr::value(u.password.clone()))
            .filter(users::Column::Id.eq(u.id))
            .exec(self.conn())
            .await?;
        Ok(())
    }

    #[instrument(skip_all)]
    async fn delete_user(&self, u: &User) -> DbResult<()> {
        use entities::{history, sessions, store, total_history_count_user as thcu, users};
        let conn = self.conn();
        // Wrapped in a transaction — the previous per-statement version could leave a
        // half-deleted user behind if a statement failed midway.
        let txn = conn.begin().await?;
        sessions::Entity::delete_many()
            .filter(sessions::Column::UserId.eq(u.id))
            .exec(&txn)
            .await?;
        history::Entity::delete_many().filter(history::Column::UserId.eq(u.id)).exec(&txn).await?;
        store::Entity::delete_many().filter(store::Column::UserId.eq(u.id)).exec(&txn).await?;
        // `total_history_count_user` is a postgres-only table (trigger-maintained);
        // mysql/sqlite have no such table, so we only purge it on pg.
        if conn.get_database_backend() == DbBackend::Postgres {
            thcu::Entity::delete_many().filter(thcu::Column::UserId.eq(u.id)).exec(&txn).await?;
        }
        users::Entity::delete_many().filter(users::Column::Id.eq(u.id)).exec(&txn).await?;
        txn.commit().await?;
        Ok(())
    }

    #[instrument(skip_all)]
    async fn delete_store(&self, user: &User) -> DbResult<()> {
        use entities::store;
        store::Entity::delete_many()
            .filter(store::Column::UserId.eq(user.id))
            .exec(self.conn())
            .await?;
        Ok(())
    }

    #[instrument(skip_all)]
    async fn add_records(&self, user: &User, records: &[Record<EncryptedData>]) -> DbResult<()> {
        use entities::store;
        if records.is_empty() {
            return Ok(());
        }

        let conn = self.conn();
        let on_conflict = record_upsert(conn.get_database_backend());

        let txn = conn.begin().await?;
        for chunk in records.chunks(ADD_RECORDS_CHUNK) {
            let rows = chunk.iter().map(|r| store::ActiveModel {
                id: Set(atuin_common::utils::uuid_v7()),
                client_id: Set(r.id.0),
                host: Set(r.host.id.0),
                idx: Set(r.idx as i64),
                // throwing away some data, but i64 is still big in terms of time
                timestamp: Set(r.timestamp as i64),
                version: Set(r.version.as_str().to_owned()),
                tag: Set(r.tag.as_str().to_owned()),
                data: Set(r.data.raw.clone()),
                cek: Set(r.data.cek.clone()),
                user_id: Set(user.id),
            });

            // One INSERT per chunk instead of the old row-at-a-time loop.
            store::Entity::insert_many(rows)
                .on_conflict(on_conflict.clone())
                .exec_without_returning(&txn)
                .await?;
        }
        txn.commit().await?;

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
        use entities::store;
        tracing::debug!("{:?} - {:?} - {:?}", series.host_id, series.tag, start);
        let start = start.unwrap_or(0);

        let rows = store::Entity::find()
            .filter(store::Column::UserId.eq(user.id))
            .filter(store::Column::Tag.eq(series.tag.as_str()))
            .filter(store::Column::Host.eq(series.host_id.0))
            .filter(store::Column::Idx.gte(start as i64))
            .order_by_asc(store::Column::Idx)
            .limit(count)
            .all(self.conn())
            .await?;

        Ok(rows.into_iter().map(record_from_model).collect())
    }

    // Return the tail record ID for each store, so (HostID, Tag, TailRecordID)
    #[instrument(skip_all)]
    async fn status(&self, user: &User) -> DbResult<RecordStatus> {
        use entities::store;
        let rows = store::Entity::find()
            .select_only()
            .column(store::Column::Host)
            .column(store::Column::Tag)
            .column_as(store::Column::Idx.max(), "idx")
            .filter(store::Column::UserId.eq(user.id))
            .group_by(store::Column::Host)
            .group_by(store::Column::Tag)
            .into_model::<StatusRow>()
            .all(self.conn())
            .await?;

        Ok(RecordStatus::from_points(
            rows.into_iter().map(|r| {
                (RecordSeriesKey::new(HostId(r.host), RecordTag::from(r.tag)), r.idx as u64)
            }),
        ))
    }
}

#[cfg(test)]
mod tests {
    use sea_orm::{DbBackend, QueryTrait};

    use super::*;

    fn upsert_sql(backend: DbBackend) -> String {
        let row = entities::store::ActiveModel {
            id: Set(atuin_common::utils::uuid_v7()),
            client_id: Set(atuin_common::utils::uuid_v7()),
            host: Set(atuin_common::utils::uuid_v7()),
            idx: Set(1),
            timestamp: Set(1),
            version: Set("2".into()),
            tag: Set("history".into()),
            data: Set("d".into()),
            cek: Set("c".into()),
            user_id: Set(1),
        };
        entities::store::Entity::insert_many([row])
            .on_conflict(record_upsert(backend))
            .build(backend)
            .sql
    }

    /// The `add_records` upsert must render as the same clause each hand-written backend
    /// used to emit. This also guards the sea-query 1.0 bug where a plain `do_nothing()`
    /// renders the invalid `ON DUPLICATE KEY IGNORE` for MySQL (see [`record_upsert`]).
    #[test]
    fn add_records_upsert_is_dialect_correct() {
        let pg = upsert_sql(DbBackend::Postgres);
        assert!(pg.contains("ON CONFLICT") && pg.contains("DO NOTHING"), "{pg}");

        let sqlite = upsert_sql(DbBackend::Sqlite);
        assert!(sqlite.contains("ON CONFLICT") && sqlite.contains("DO NOTHING"), "{sqlite}");

        let mysql = upsert_sql(DbBackend::MySql);
        assert!(mysql.contains("ON DUPLICATE KEY UPDATE"), "{mysql}");
        assert!(
            !mysql.contains("IGNORE"),
            "regression: invalid `ON DUPLICATE KEY IGNORE`: {mysql}"
        );
    }
}
