//! Utilities for operating on databases.

#[cfg(feature = "sqlite")]
pub mod sqlite;

use sqlx::query::{Query, QueryAs, QueryScalar};
use sqlx::{Database, FromRow, SqlSafeStr};

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

#[cfg(all(test, feature = "sqlite"))]
mod tests {
    use proptest::prelude::*;
    use rstest::{fixture, rstest};
    use sqlx::Sqlite;
    use sqlx::pool::PoolConnection;

    use super::*;
    use crate::db::sqlite::Sqlite as AtuinSqlite;

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
