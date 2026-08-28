//! Utilities for operating on databases.

#[cfg(feature = "sqlite")]
pub mod sqlite;

use std::borrow::Borrow;
use std::fmt;
use std::ops::Deref;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sqlx::query::{Query, QueryAs, QueryScalar};
use sqlx::{Database, FromRow, SqlSafeStr};
use thiserror::Error;
use url::Url;

use crate::string::FormatSafeUrlExt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BannedPattern {
    StarGlob,
    Alter,
}

impl BannedPattern {
    fn test(sql: &str) -> Option<Self> {
        sql.split_whitespace().find_map(|raw| {
            let token = raw.trim_matches(|c: char| matches!(c, ',' | '(' | ')' | ';'));
            let upper = token.to_ascii_uppercase();

            if upper == "ALTER" {
                Some(Self::Alter)
            } else if token == "*" || upper.ends_with(".*") {
                Some(Self::StarGlob)
            } else {
                None
            }
        })
    }

    fn reason(self) -> &'static str {
        match self {
            Self::StarGlob => "`*` is banned in queries due to being a footgun. be explicit.",
            Self::Alter => "`ALTER` is banned in queries due to being a footgun. use migrations.",
        }
    }
}

/// Utility designed to prevent foot-gunny SQL queries.
#[track_caller]
fn debug_sanity_check_query(sql: &str) {
    if cfg!(debug_assertions)
        && let Some(pattern) = BannedPattern::test(sql)
    {
        panic!("{}.\n  query: {sql}", pattern.reason());
    }
}

/// Equivalent to [`sqlx::query()`].
pub fn query<'a, DB>(sql: impl SqlSafeStr) -> Query<'a, DB, <DB as Database>::Arguments>
where
    DB: Database,
{
    let sql = sql.into_sql_str();
    debug_sanity_check_query(sql.as_str());
    #[allow(clippy::disallowed_methods)]
    sqlx::query(sql)
}

/// Equivalent to [`sqlx::query_as()`].
pub fn query_as<'q, DB, O>(sql: impl SqlSafeStr) -> QueryAs<'q, DB, O, <DB as Database>::Arguments>
where
    DB: Database,
    O: for<'r> FromRow<'r, DB::Row>,
{
    let sql = sql.into_sql_str();
    debug_sanity_check_query(sql.as_str());
    #[allow(clippy::disallowed_methods)]
    sqlx::query_as(sql)
}

/// Equivalent to [`sqlx::query_scalar()`].
pub fn query_scalar<'q, DB, O>(
    sql: impl SqlSafeStr,
) -> QueryScalar<'q, DB, O, <DB as Database>::Arguments>
where
    DB: Database,
    (O,): for<'r> FromRow<'r, DB::Row>,
{
    let sql = sql.into_sql_str();
    debug_sanity_check_query(sql.as_str());
    #[allow(clippy::disallowed_methods)]
    sqlx::query_scalar(sql)
}

#[macro_export]
macro_rules! __atuin_db_migrate {
    ($pool:expr, $dir:literal) => {{
        let pool = $pool;
        async move {
            #[allow(clippy::disallowed_macros)]
            ::sqlx::migrate!($dir).run(pool).await?;
            let mut conn = ::sqlx::Acquire::acquire(pool)
                .await
                .map_err(::sqlx::migrate::MigrateError::Execute)?;
            // Unfortunately this is necessary. Sqlx caches statements, so if you do something like
            // "SELECT * FROM foobar" where foobar has columns foo and bar, sqlx will cache the
            // "compiled" statement.
            //
            // This results in nastiness with migrations -- you _just_ ran a migration, so you need
            // to effectively clear the cached statements:
            //
            // See https://github.com/transact-rs/sqlx/issues/2517
            ::sqlx::Connection::clear_cached_statements(&mut *conn).await?;
            ::core::result::Result::<(), ::sqlx::migrate::MigrateError>::Ok(())
        }
    }};
}

pub use __atuin_db_migrate as migrate;

/// A sqlite connection string.
#[derive(Clone, PartialEq, Eq)]
pub struct SqliteDbUrl<T: Borrow<str> = String>(pub T);

/// A postgres connection URL.
#[derive(Clone, PartialEq, Eq)]
pub struct PostgresDbUrl<T: Borrow<Url> = Url>(pub T);

/// A mysql connection URL.
#[derive(Clone, PartialEq, Eq)]
pub struct MysqlDbUrl<T: Borrow<Url> = Url>(pub T);

impl<T: Borrow<str>> SqliteDbUrl<T> {
    /// The connection string as sqlx expects it, byte-for-byte.
    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.borrow()
    }
}

impl<T: Borrow<Url>> PostgresDbUrl<T> {
    #[must_use]
    pub fn url(&self) -> &Url {
        self.0.borrow()
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.borrow().as_str()
    }
}

impl<T: Borrow<Url>> MysqlDbUrl<T> {
    #[must_use]
    pub fn url(&self) -> &Url {
        self.0.borrow()
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.borrow().as_str()
    }
}

impl<T: Borrow<str>> fmt::Debug for SqliteDbUrl<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.0.borrow(), f)
    }
}

impl<T: Borrow<Url>> fmt::Debug for PostgresDbUrl<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.borrow().format_safe(f)
    }
}

impl<T: Borrow<Url>> fmt::Debug for MysqlDbUrl<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.borrow().format_safe(f)
    }
}

/// A database connection URL, tagged by backend.
#[derive(Clone, PartialEq, Eq, derive_more::Debug)]
pub enum DbUrl<S = String, U = Url>
where
    S: Borrow<str>,
    U: Borrow<Url>,
{
    #[debug("{_0:?}")]
    Sqlite(SqliteDbUrl<S>),
    #[debug("{_0:?}")]
    Postgres(PostgresDbUrl<U>),
    #[debug("{_0:?}")]
    Mysql(MysqlDbUrl<U>),
}

/// An owned [`DbUrl`] — what [`FromStr`], serde, and config produce.
pub type OwnedDbUrl = DbUrl;

/// A borrowed view into an [`OwnedDbUrl`], produced by [`OwnedDbUrl::as_view`].
pub type DbUrlView<'a> = DbUrl<&'a str, &'a Url>;

impl<S: Borrow<str>, U: Borrow<Url>> DbUrl<S, U> {
    /// The connection string as sqlx expects it, byte-for-byte.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::Sqlite(SqliteDbUrl(s)) => s.borrow(),
            Self::Postgres(PostgresDbUrl(u)) | Self::Mysql(MysqlDbUrl(u)) => u.borrow().as_str(),
        }
    }
}

impl<S: Borrow<str>, U: Borrow<Url>> Deref for DbUrl<S, U> {
    type Target = str;

    fn deref(&self) -> &str {
        self.as_str()
    }
}

impl OwnedDbUrl {
    /// Borrow this URL as a [`DbUrlView`], without cloning.
    #[must_use]
    pub fn as_view(&self) -> DbUrlView<'_> {
        match self {
            Self::Sqlite(SqliteDbUrl(s)) => DbUrl::Sqlite(SqliteDbUrl(s.as_str())),
            Self::Postgres(PostgresDbUrl(u)) => DbUrl::Postgres(PostgresDbUrl(u)),
            Self::Mysql(MysqlDbUrl(u)) => DbUrl::Mysql(MysqlDbUrl(u)),
        }
    }
}

/// The scheme of a database URL was not one we recognise.
#[derive(Debug, Error)]
pub enum DbUrlParseError {
    #[error("unrecognised database scheme)")]
    UnknownScheme,
    #[error("invalid database url: {_0}")]
    InvalidUrl(#[from] url::ParseError),
}

impl FromStr for OwnedDbUrl {
    type Err = DbUrlParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.starts_with("postgres://") || s.starts_with("postgresql://") {
            Ok(Self::Postgres(PostgresDbUrl(Url::parse(s)?)))
        } else if s.starts_with("mysql://") {
            Ok(Self::Mysql(MysqlDbUrl(Url::parse(s)?)))
        } else if s.starts_with("sqlite:") {
            Ok(Self::Sqlite(SqliteDbUrl(s.to_owned())))
        } else {
            Err(DbUrlParseError::UnknownScheme)
        }
    }
}

impl Serialize for OwnedDbUrl {
    fn serialize<Ser: Serializer>(&self, serializer: Ser) -> Result<Ser::Ok, Ser::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for OwnedDbUrl {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        raw.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(all(test, feature = "sqlite"))]
mod tests {
    use proptest::prelude::*;
    use rstest::{fixture, rstest};
    use sqlx::Sqlite;
    use sqlx::pool::PoolConnection;

    use super::*;
    use crate::db::sqlite::Sqlite as AtuinSqlite;

    #[rstest]
    #[case::postgres_password(
        "postgres://user:hunter2@host:5432/atuin",
        r#""postgres://user:****@host:5432/atuin""#
    )]
    #[case::mysql_password(
        "mysql://root:hunter2@127.0.0.1/atuin",
        r#""mysql://root:****@127.0.0.1/atuin""#
    )]
    // A URL without a password is printed as-is; `****` is never fabricated.
    #[case::postgres_no_password("postgres://host/atuin", r#""postgres://host/atuin""#)]
    // sqlite is never parsed as a URL, so nothing is redacted or reshaped.
    #[case::sqlite(
        "sqlite:///var/lib/atuin/atuin.db?mode=rwc",
        r#""sqlite:///var/lib/atuin/atuin.db?mode=rwc""#
    )]
    fn db_url_debug_redacts_passwords(#[case] uri: &str, #[case] expected_debug: &str) {
        let url: OwnedDbUrl = uri.parse().unwrap();
        assert_eq!(format!("{url:?}"), expected_debug);
        assert!(!format!("{url:?}").contains("hunter2"), "password leaked into Debug");
    }

    #[rstest]
    #[case::postgres("postgres://u:p@h/db")]
    #[case::mysql("mysql://u:p@h/db")]
    #[case::sqlite("sqlite://:memory:")]
    fn db_url_as_str_matches_view(#[case] uri: &str) {
        let owned: OwnedDbUrl = uri.parse().unwrap();
        assert_eq!(owned.as_str(), owned.as_view().as_str());
    }

    #[rstest]
    #[case::path("sqlite:///var/lib/atuin/atuin.db?mode=rwc")]
    #[case::memory("sqlite://:memory:")]
    fn sqlite_url_reaches_sqlx_byte_for_byte(#[case] uri: &str) {
        let url: OwnedDbUrl = uri.parse().unwrap();
        assert_eq!(url.as_str(), uri);
    }

    #[rstest]
    #[case::redis("redis://localhost")]
    #[case::bare("not-a-database-url")]
    fn db_url_rejects_unknown_scheme(#[case] uri: &str) {
        assert!(matches!(uri.parse::<OwnedDbUrl>(), Err(DbUrlParseError::UnknownScheme)));
    }

    #[test]
    fn db_url_serde_round_trips_through_the_real_connection_string() {
        let owned: OwnedDbUrl = "postgres://user:hunter2@host/atuin".parse().unwrap();
        let json = serde_json::to_string(&owned).unwrap();
        // serialized form is the real connection string, not the redacted debug
        assert_eq!(json, r#""postgres://user:hunter2@host/atuin""#);
        assert_eq!(serde_json::from_str::<OwnedDbUrl>(&json).unwrap(), owned);
    }

    #[rstest]
    #[case::bare_star("select * from history", Some(BannedPattern::StarGlob))]
    #[case::many_spaces_star("select      *  from history", Some(BannedPattern::StarGlob))]
    #[case::tab_star("select\t*\tfrom history", Some(BannedPattern::StarGlob))]
    #[case::newline_star("select\n  *\n  from history", Some(BannedPattern::StarGlob))]
    #[case::qualified_star("select h.* from history h", Some(BannedPattern::StarGlob))]
    #[case::star_amongst_columns("select id, * from history", Some(BannedPattern::StarGlob))]
    #[case::alter("alter table history add column x integer", Some(BannedPattern::Alter))]
    #[case::alter_upper("ALTER TABLE history ADD COLUMN x integer", Some(BannedPattern::Alter))]
    #[case::explicit("select id, timestamp, command from history", None)]
    #[case::count_star("select count(*) from history", None)]
    #[case::pragma("PRAGMA wal_checkpoint(RESTART)", None)]
    #[case::scalar_fn("select sqlite_version()", None)]
    #[case::insert("insert into t (a, b) values (?1, ?2)", None)]
    #[case::migration_only_add("add column shell text", None)]
    fn banned_pattern_classifies(#[case] sql: &str, #[case] expected: Option<BannedPattern>) {
        assert_eq!(BannedPattern::test(sql), expected);
    }

    proptest! {
        #[test]
        fn explicit_column_lists_are_allowed(cols in prop::collection::vec("[b-z][a-z0-9_]{0,7}", 1..8)) {
            let sql = format!("select {} from t", cols.join(", "));
            prop_assert_eq!(BannedPattern::test(&sql), None);
        }

        #[test]
        fn a_bare_star_selector_is_always_rejected(lead in prop::collection::vec("[b-z][a-z0-9_]{0,7}", 0..4)) {
            let mut fields = lead;
            fields.push("*".to_owned());
            let sql = format!("select {} from t", fields.join(", "));
            prop_assert_eq!(BannedPattern::test(&sql), Some(BannedPattern::StarGlob));
        }

        #[test]
        fn a_qualified_star_is_always_rejected(alias in "[b-z][a-z0-9_]{0,7}") {
            let sql = format!("select {alias}.* from t {alias}");
            prop_assert_eq!(BannedPattern::test(&sql), Some(BannedPattern::StarGlob));
        }
    }

    struct PrimedDb {
        _sqlite: AtuinSqlite,
        conn: PoolConnection<Sqlite>,
    }

    #[fixture]
    async fn primed_db() -> PrimedDb {
        let sqlite = AtuinSqlite::builder_in_memory().open().await.unwrap();
        let mut conn = sqlite.pool().acquire().await.unwrap();

        query::<Sqlite>("create table t (a integer, b integer)").execute(&mut *conn).await.unwrap();
        query::<Sqlite>("insert into t (a, b) values (1, 2)").execute(&mut *conn).await.unwrap();
        query_as::<Sqlite, (i64, i64)>("select a, b from t").fetch_all(&mut *conn).await.unwrap();

        PrimedDb {
            _sqlite: sqlite,
            conn,
        }
    }

    #[allow(clippy::disallowed_methods)]
    async fn add_columns(conn: &mut PoolConnection<Sqlite>, columns: &[&str]) {
        for column in columns {
            sqlx::query(sqlx::AssertSqlSafe(format!("alter table t add column {column} integer")))
                .execute(&mut **conn)
                .await
                .unwrap();
        }
    }

    #[rstest]
    #[case::add_one(&["c"])]
    #[case::add_several(&["c", "d", "e"])]
    #[tokio::test]
    async fn explicit_columns_survive_add_column(
        #[future] primed_db: PrimedDb,
        #[case] added: &[&str],
    ) {
        let mut db = primed_db.await;

        add_columns(&mut db.conn, added).await;

        let rows: Vec<(i64, i64)> = query_as::<Sqlite, (i64, i64)>("select a, b from t")
            .fetch_all(&mut *db.conn)
            .await
            .unwrap();
        assert_eq!(rows, vec![(1, 2)]);
    }
}
