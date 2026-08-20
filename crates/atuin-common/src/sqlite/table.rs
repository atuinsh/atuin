use std::marker::PhantomData;

use sqlx::{Encode, QueryBuilder, Sqlite as SqliteDb, Type};

use super::Sqlite;

pub enum Conflict {
    Ignore,
    Replace,
    Upsert,
}

pub enum ColKind {
    Bind,
    Expr(&'static str),
}

pub struct Col {
    pub name: &'static str,
    pub key: bool,
    pub kind: ColKind,
}

impl Col {
    #[allow(clippy::self_named_constructors)]
    pub const fn col(name: &'static str) -> Self {
        Self {
            name,
            key: false,
            kind: ColKind::Bind,
        }
    }

    pub const fn key(name: &'static str) -> Self {
        Self {
            name,
            key: true,
            kind: ColKind::Bind,
        }
    }

    pub const fn expr(name: &'static str, sql: &'static str) -> Self {
        Self {
            name,
            key: false,
            kind: ColKind::Expr(sql),
        }
    }
}

pub struct Schema {
    pub name: &'static str,
    pub columns: &'static [Col],
    pub conflict: Conflict,
}

impl Schema {
    pub const fn col_count(&self) -> usize {
        self.columns.len()
    }

    pub const fn bind_col_count(&self) -> usize {
        let mut n = 0;
        let mut i = 0;
        while i < self.columns.len() {
            if matches!(self.columns[i].kind, ColKind::Bind) {
                n += 1;
            }
            i += 1;
        }
        n
    }
}

pub trait Table {
    const SCHEMA: Schema;
    fn bind_row(&self, sep: &mut sqlx::query_builder::Separated<'_, SqliteDb, &'static str>);
}

/// Const-time membership test: is `name` one of `keys`?
///
/// Used by the [`table!`] macro to resolve each column's `key` flag against the
/// declared `key:` list at macro-expansion time, so the column ordering at the
/// macro surface is irrelevant. Kept `const` so it can run inside the `SCHEMA`
/// associated-const initializer.
#[must_use]
pub const fn is_key(name: &str, keys: &[&str]) -> bool {
    let name = name.as_bytes();
    let mut i = 0;
    while i < keys.len() {
        let k = keys[i].as_bytes();
        if k.len() == name.len() {
            let mut j = 0;
            let mut eq = true;
            while j < name.len() {
                if name[j] != k[j] {
                    eq = false;
                    break;
                }
                j += 1;
            }
            if eq {
                return true;
            }
        }
        i += 1;
    }
    false
}

/// Emit an `impl $crate::sqlite::Table for T` from a compact declaration.
///
/// ```ignore
/// table!(Kv3 {
///     name: "kv3",
///     key: ["ns", "k"],          // or a single `key: "id"`
///     conflict: upsert,          // ignore | replace | upsert
///     columns: {
///         ns => |e| e.ns.as_str(),                 // Bind column
///         k  => |e| e.k.as_str(),
///         v  => |e| e.v.as_str(),
///         at => sql("strftime('%s','now')"),       // Expr column (no bind)
///     },
/// });
/// ```
///
/// Column names are idents (stringified for the schema/SQL). A column's `key`
/// flag is `true` iff its name appears in the `key:` list (resolved by name via
/// [`is_key`], so surface order does not matter). Every path the macro emits is
/// fully qualified (`$crate::sqlite::*`, `sqlx::*`) so it expands in crates that
/// import nothing.
#[macro_export]
macro_rules! table {
    // ---- public entry point -------------------------------------------------
    (
        $ty:ty {
            name: $name:literal,
            key: $key:tt,
            conflict: $conflict:ident,
            columns: { $($cols:tt)* } $(,)?
        } $(,)?
    ) => {
        impl $crate::sqlite::Table for $ty {
            const SCHEMA: $crate::sqlite::Schema = $crate::sqlite::Schema {
                name: $name,
                columns: $crate::table!(@cols_array $key; $($cols)*),
                conflict: $crate::table!(@conflict $conflict),
            };

            fn bind_row(
                &self,
                sep: &mut sqlx::query_builder::Separated<'_, sqlx::Sqlite, &'static str>,
            ) {
                $crate::table!(@bind_each self, sep; $($cols)*);
            }
        }
    };

    // ---- conflict variant ---------------------------------------------------
    (@conflict ignore) => { $crate::sqlite::Conflict::Ignore };
    (@conflict replace) => { $crate::sqlite::Conflict::Replace };
    (@conflict upsert) => { $crate::sqlite::Conflict::Upsert };

    // ---- key list -> `&[&str]` ----------------------------------------------
    (@keys [ $($k:literal),* $(,)? ]) => { &[ $($k),* ] };
    (@keys $k:literal) => { &[ $k ] };

    // ---- columns -> `&[Col]` ------------------------------------------------
    // Accumulator muncher: each column is folded into `[$($acc),*]`, then the
    // whole array is emitted at once (a macro in expression position must expand
    // to a single expression, so we cannot emit `Col, Col` piecewise).
    (@cols_array $key:tt; $($cols:tt)*) => {
        $crate::table!(@cols_acc $key; []; $($cols)*)
    };

    // done
    (@cols_acc $key:tt; [$($acc:expr),* $(,)?]; ) => {
        &[ $($acc),* ]
    };
    // Expr column (optional trailing rest)
    (@cols_acc $key:tt; [$($acc:expr),* $(,)?]; $cname:ident => sql($sql:literal) $(, $($rest:tt)*)? ) => {
        $crate::table!(@cols_acc $key;
            [ $($acc,)* $crate::sqlite::Col::expr(stringify!($cname), $sql) ];
            $($($rest)*)?
        )
    };
    // Bind column (optional trailing rest)
    (@cols_acc $key:tt; [$($acc:expr),* $(,)?]; $cname:ident => | $arg:ident | $body:expr $(, $($rest:tt)*)? ) => {
        $crate::table!(@cols_acc $key;
            [ $($acc,)* $crate::sqlite::Col {
                name: stringify!($cname),
                key: $crate::sqlite::is_key(stringify!($cname), $crate::table!(@keys $key)),
                kind: $crate::sqlite::ColKind::Bind,
            } ];
            $($($rest)*)?
        )
    };

    // ---- columns -> bind_row body -------------------------------------------
    // Recursive muncher in statement position (a macro-as-statement may expand
    // to several statements, so piecewise emission is fine here).
    (@bind_each $this:expr, $sep:ident; ) => { };
    // Expr column: push the literal SQL, no bind.
    (@bind_each $this:expr, $sep:ident; $cname:ident => sql($sql:literal) $(, $($rest:tt)*)? ) => {
        $sep.push($sql);
        $crate::table!(@bind_each $this, $sep; $($($rest)*)?)
    };
    // Bind column: evaluate the accessor against `self` and bind it.
    (@bind_each $this:expr, $sep:ident; $cname:ident => | $arg:ident | $body:expr $(, $($rest:tt)*)? ) => {
        $sep.push_bind({ let $arg: &Self = $this; $body });
        $crate::table!(@bind_each $this, $sep; $($($rest)*)?)
    };
}

#[derive(Debug, Clone)]
pub struct TableView<T> {
    sqlite: Sqlite,
    _t: PhantomData<T>,
}

impl<T> TableView<T> {
    pub fn new(sqlite: Sqlite) -> Self {
        Self {
            sqlite,
            _t: PhantomData,
        }
    }

    pub fn sqlite(&self) -> &Sqlite {
        &self.sqlite
    }
}

/// Types that can appear as a single key column value bound via `push_bind`.
///
/// This is a sealed, closed set of concrete scalar types (rather than a
/// blanket impl over `Encode + Type`) so that the single-value [`KeyBind`]
/// impl below does not coherence-conflict (E0119) with the tuple impls:
/// rustc cannot rule out that `sqlx` might implement `Encode`/`Type` for
/// tuples in a future version, but it *can* see that this sealed trait is
/// only ever implemented for the scalar types listed here.
mod sealed {
    pub trait KeyScalar {}
}
use sealed::KeyScalar;

macro_rules! impl_key_scalar {
    ($($t:ty),* $(,)?) => {
        $(impl KeyScalar for $t {})*
    };
}

impl_key_scalar!(&str, String, i64, i32, u32, u64, bool);

/// Push `col0 = ? and col1 = ? …`, binding the key values in order.
pub trait KeyBind {
    fn push_where(self, qb: &mut QueryBuilder<SqliteDb>, cols: &[&str]);
}

impl<A> KeyBind for A
where
    A: KeyScalar + for<'a> Encode<'a, SqliteDb> + Type<SqliteDb> + Send,
{
    fn push_where(self, qb: &mut QueryBuilder<SqliteDb>, cols: &[&str]) {
        qb.push(cols[0]).push(" = ").push_bind(self);
    }
}

impl<A, B> KeyBind for (A, B)
where
    A: for<'a> Encode<'a, SqliteDb> + Type<SqliteDb> + Send,
    B: for<'a> Encode<'a, SqliteDb> + Type<SqliteDb> + Send,
{
    fn push_where(self, qb: &mut QueryBuilder<SqliteDb>, cols: &[&str]) {
        qb.push(cols[0]).push(" = ").push_bind(self.0);
        qb.push(" and ").push(cols[1]).push(" = ").push_bind(self.1);
    }
}

impl<A, B, C> KeyBind for (A, B, C)
where
    A: for<'a> Encode<'a, SqliteDb> + Type<SqliteDb> + Send,
    B: for<'a> Encode<'a, SqliteDb> + Type<SqliteDb> + Send,
    C: for<'a> Encode<'a, SqliteDb> + Type<SqliteDb> + Send,
{
    fn push_where(self, qb: &mut QueryBuilder<SqliteDb>, cols: &[&str]) {
        qb.push(cols[0]).push(" = ").push_bind(self.0);
        qb.push(" and ").push(cols[1]).push(" = ").push_bind(self.1);
        qb.push(" and ").push(cols[2]).push(" = ").push_bind(self.2);
    }
}

fn insert_prefix<T: Table>() -> String {
    let s = T::SCHEMA;
    let verb = match s.conflict {
        Conflict::Ignore => "insert or ignore into ",
        Conflict::Replace => "insert or replace into ",
        Conflict::Upsert => "insert into ",
    };
    let cols = s.columns.iter().map(|c| c.name).collect::<Vec<_>>().join(", ");
    format!("{verb}{}({cols}) ", s.name)
}

fn upsert_suffix<T: Table>() -> String {
    let s = T::SCHEMA;
    let keys = s.columns.iter().filter(|c| c.key).map(|c| c.name).collect::<Vec<_>>().join(", ");
    let sets = s
        .columns
        .iter()
        .filter(|c| !c.key)
        .map(|c| format!("{n} = excluded.{n}", n = c.name))
        .collect::<Vec<_>>()
        .join(", ");
    format!(" on conflict ({keys}) do update set {sets}")
}

impl<T: Table> TableView<T> {
    pub async fn insert_bulk_tx<'a>(
        &self,
        tx: &mut sqlx::Transaction<'_, SqliteDb>,
        items: impl IntoIterator<Item = &'a T>,
    ) -> sqlx::Result<()>
    where
        T: 'a,
    {
        let mut it = items.into_iter().peekable();
        if it.peek().is_none() {
            return Ok(());
        }

        let bind_cols = T::SCHEMA.bind_col_count();
        debug_assert!(
            bind_cols > 0,
            "Table::SCHEMA for {} has no Bind columns; insert_bulk_tx cannot compute a chunk size",
            T::SCHEMA.name
        );
        let per = (self.sqlite.info().await.variable_number_limit / bind_cols.max(1)).max(1);
        let prefix = insert_prefix::<T>();
        let suffix = matches!(T::SCHEMA.conflict, Conflict::Upsert).then(upsert_suffix::<T>);

        while it.peek().is_some() {
            let mut qb = sqlx::QueryBuilder::<SqliteDb>::new(prefix.clone());
            qb.push_values(it.by_ref().take(per), |mut sep, item: &T| {
                item.bind_row(&mut sep);
            });
            if let Some(suf) = &suffix {
                qb.push(suf);
            }
            qb.build().execute(&mut **tx).await?;
        }
        Ok(())
    }

    pub async fn insert_bulk<'a>(&self, items: impl IntoIterator<Item = &'a T>) -> sqlx::Result<()>
    where
        T: 'a,
    {
        let mut tx = self.sqlite.pool().begin().await?;
        self.insert_bulk_tx(&mut tx, items).await?;
        tx.commit().await
    }

    pub async fn insert_one(&self, item: &T) -> sqlx::Result<()> {
        self.insert_bulk(std::iter::once(item)).await
    }

    fn key_cols() -> Vec<&'static str> {
        T::SCHEMA.columns.iter().filter(|c| c.key).map(|c| c.name).collect()
    }

    pub async fn delete<K: KeyBind>(&self, key: K) -> sqlx::Result<()> {
        let mut qb =
            sqlx::QueryBuilder::<SqliteDb>::new(format!("delete from {} where ", T::SCHEMA.name));
        key.push_where(&mut qb, &Self::key_cols());
        qb.build().execute(self.sqlite.pool()).await?;
        Ok(())
    }

    pub async fn delete_all(&self) -> sqlx::Result<()> {
        sqlx::query(sqlx::AssertSqlSafe(format!("delete from {}", T::SCHEMA.name)))
            .execute(self.sqlite.pool())
            .await?;
        Ok(())
    }
}

impl<T> TableView<T>
where
    T: Table + for<'r> sqlx::FromRow<'r, sqlx::sqlite::SqliteRow> + Send + Unpin,
{
    pub async fn get<K: KeyBind>(&self, key: K) -> sqlx::Result<Option<T>> {
        let mut qb =
            sqlx::QueryBuilder::<SqliteDb>::new(format!("select * from {} where ", T::SCHEMA.name));
        key.push_where(&mut qb, &Self::key_cols());
        qb.build_query_as::<T>().fetch_optional(self.sqlite.pool()).await
    }

    /// Stream all rows. Holds one pooled connection for the stream's
    /// lifetime; callers `.try_collect()` if they want a `Vec`.
    pub fn all(&self) -> impl futures::Stream<Item = sqlx::Result<T>> + Send + '_ {
        sqlx::query_as(sqlx::AssertSqlSafe(format!("select * from {}", T::SCHEMA.name)))
            .fetch(self.sqlite.pool())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // NOTE: this block deliberately does NOT `use` any of the sqlite types
    // (`Table`/`Schema`/`Col`/`Conflict`/`ColKind`). It relies purely on the
    // fully-qualified paths the macro emits (`$crate::sqlite::*`, `sqlx::*`),
    // so a missing qualification inside `table!` fails to compile here.
    mod macro_hygiene {
        struct Kv3 {
            ns: String,
            k: String,
            v: String,
        }
        crate::table!(Kv3 {
            name: "kv3",
            key: ["ns", "k"],
            conflict: upsert,
            columns: {
                ns => |e| e.ns.as_str(),
                k  => |e| e.k.as_str(),
                v  => |e| e.v.as_str(),
            },
        });

        // A table with a composite key AND an Expr column, defined via the macro.
        struct Event {
            host: String,
            id: String,
            body: String,
        }
        crate::table!(Event {
            name: "events",
            key: ["host", "id"],
            conflict: ignore,
            columns: {
                host    => |e| e.host.as_str(),
                id      => |e| e.id.as_str(),
                body    => |e| e.body.as_str(),
                created => sql("strftime('%s','now')"),
            },
        });

        // A single (non-list) key with `replace` conflict.
        struct Single {
            id: String,
            val: String,
        }
        crate::table!(Single {
            name: "single",
            key: "id",
            conflict: replace,
            columns: {
                id  => |e| e.id.as_str(),
                val => |e| e.val.as_str(),
            }
        });

        #[test]
        fn macro_builds_schema() {
            use crate::sqlite::{Conflict, Table};
            assert_eq!(Kv3::SCHEMA.name, "kv3");
            assert_eq!(Kv3::SCHEMA.col_count(), 3);
            assert_eq!(Kv3::SCHEMA.columns[0].name, "ns");
            assert!(Kv3::SCHEMA.columns[0].key);
            assert!(Kv3::SCHEMA.columns[1].key);
            assert!(!Kv3::SCHEMA.columns[2].key);
            assert!(matches!(Kv3::SCHEMA.conflict, Conflict::Upsert));
        }

        #[test]
        fn macro_composite_key_and_expr_col() {
            use crate::sqlite::{ColKind, Conflict, Table};
            assert_eq!(Event::SCHEMA.name, "events");
            assert_eq!(Event::SCHEMA.col_count(), 4);
            // composite key membership resolved by column name, order-independent
            assert!(Event::SCHEMA.columns[0].key); // host
            assert!(Event::SCHEMA.columns[1].key); // id
            assert!(!Event::SCHEMA.columns[2].key); // body
            assert!(!Event::SCHEMA.columns[3].key); // created (expr, never key)
            // only the three Bind columns count toward bind params
            assert_eq!(Event::SCHEMA.bind_col_count(), 3);
            assert!(matches!(Event::SCHEMA.columns[3].kind, ColKind::Expr("strftime('%s','now')")));
            assert!(matches!(Event::SCHEMA.conflict, Conflict::Ignore));
        }

        #[test]
        fn macro_single_key() {
            use crate::sqlite::{Conflict, Table};
            assert_eq!(Single::SCHEMA.name, "single");
            assert!(Single::SCHEMA.columns[0].key);
            assert!(!Single::SCHEMA.columns[1].key);
            assert!(matches!(Single::SCHEMA.conflict, Conflict::Replace));
        }
    }

    struct Toy;
    impl Table for Toy {
        const SCHEMA: Schema = Schema {
            name: "toy",
            columns: &[
                Col {
                    name: "id",
                    key: true,
                    kind: ColKind::Bind,
                },
                Col {
                    name: "body",
                    key: false,
                    kind: ColKind::Bind,
                },
                Col {
                    name: "at",
                    key: false,
                    kind: ColKind::Expr("strftime('%s','now')"),
                },
            ],
            conflict: Conflict::Upsert,
        };
        fn bind_row(&self, sep: &mut sqlx::query_builder::Separated<'_, SqliteDb, &'static str>) {
            sep.push_bind("x"); // id
            sep.push_bind("y"); // body
            sep.push("strftime('%s','now')"); // at (expr)
        }
    }

    #[test]
    fn schema_counts() {
        assert_eq!(Toy::SCHEMA.col_count(), 3);
        assert_eq!(Toy::SCHEMA.bind_col_count(), 2); // Expr excluded
        assert!(Toy::SCHEMA.columns[0].key);
        assert!(!Toy::SCHEMA.columns[1].key);
    }

    #[test]
    fn keybind_builds_where() {
        use sqlx::QueryBuilder;
        let mut qb: QueryBuilder<sqlx::Sqlite> = QueryBuilder::new("select 1 where ");
        ("ns", "k").push_where(&mut qb, &["namespace", "key"]);
        assert_eq!(qb.sql(), "select 1 where namespace = ? and key = ?");
    }

    #[tokio::test]
    async fn insert_bulk_ignore_and_upsert() {
        // Toy2: 2 bind cols + conflict Upsert on `id`
        struct Kv {
            id: String,
            val: String,
        }
        impl Table for Kv {
            const SCHEMA: Schema = Schema {
                name: "kv2",
                columns: &[Col::key("id"), Col::col("val")],
                conflict: Conflict::Upsert,
            };
            fn bind_row(
                &self,
                sep: &mut sqlx::query_builder::Separated<'_, SqliteDb, &'static str>,
            ) {
                sep.push_bind(self.id.as_str()).push_bind(self.val.as_str());
            }
        }

        let sqlite = crate::sqlite::Sqlite::builder().memory().open().await.unwrap();
        sqlx::query("create table kv2(id text primary key, val text)")
            .execute(sqlite.pool())
            .await
            .unwrap();

        let view = TableView::<Kv>::new(sqlite.clone());
        view.insert_bulk([
            &Kv {
                id: "a".into(),
                val: "1".into(),
            },
            &Kv {
                id: "b".into(),
                val: "2".into(),
            },
        ])
        .await
        .unwrap();
        view.insert_one(&Kv {
            id: "a".into(),
            val: "updated".into(),
        })
        .await
        .unwrap(); // upsert

        let n: i64 =
            sqlx::query_scalar("select count(*) from kv2").fetch_one(sqlite.pool()).await.unwrap();
        assert_eq!(n, 2);
        let v: String = sqlx::query_scalar("select val from kv2 where id='a'")
            .fetch_one(sqlite.pool())
            .await
            .unwrap();
        assert_eq!(v, "updated");
    }

    #[tokio::test]
    async fn insert_bulk_ignore_keeps_original_value() {
        struct Kv {
            id: String,
            val: String,
        }
        impl Table for Kv {
            const SCHEMA: Schema = Schema {
                name: "kv_ignore",
                columns: &[Col::key("id"), Col::col("val")],
                conflict: Conflict::Ignore,
            };
            fn bind_row(
                &self,
                sep: &mut sqlx::query_builder::Separated<'_, SqliteDb, &'static str>,
            ) {
                sep.push_bind(self.id.as_str()).push_bind(self.val.as_str());
            }
        }

        let sqlite = crate::sqlite::Sqlite::builder().memory().open().await.unwrap();
        sqlx::query("create table kv_ignore(id text primary key, val text)")
            .execute(sqlite.pool())
            .await
            .unwrap();

        let view = TableView::<Kv>::new(sqlite.clone());
        view.insert_bulk([&Kv {
            id: "a".into(),
            val: "original".into(),
        }])
        .await
        .unwrap();

        // Same key, different value: `insert or ignore` must not error and must not
        // overwrite the existing row.
        view.insert_bulk([
            &Kv {
                id: "a".into(),
                val: "clobbered".into(),
            },
            &Kv {
                id: "b".into(),
                val: "1".into(),
            },
        ])
        .await
        .unwrap();

        let n: i64 = sqlx::query_scalar("select count(*) from kv_ignore")
            .fetch_one(sqlite.pool())
            .await
            .unwrap();
        assert_eq!(n, 2);
        let v: String = sqlx::query_scalar("select val from kv_ignore where id='a'")
            .fetch_one(sqlite.pool())
            .await
            .unwrap();
        assert_eq!(v, "original");
    }

    #[tokio::test]
    async fn insert_bulk_replace_overwrites_value() {
        struct Kv {
            id: String,
            val: String,
        }
        impl Table for Kv {
            const SCHEMA: Schema = Schema {
                name: "kv_replace",
                columns: &[Col::key("id"), Col::col("val")],
                conflict: Conflict::Replace,
            };
            fn bind_row(
                &self,
                sep: &mut sqlx::query_builder::Separated<'_, SqliteDb, &'static str>,
            ) {
                sep.push_bind(self.id.as_str()).push_bind(self.val.as_str());
            }
        }

        let sqlite = crate::sqlite::Sqlite::builder().memory().open().await.unwrap();
        sqlx::query("create table kv_replace(id text primary key, val text)")
            .execute(sqlite.pool())
            .await
            .unwrap();

        let view = TableView::<Kv>::new(sqlite.clone());
        view.insert_bulk([&Kv {
            id: "a".into(),
            val: "original".into(),
        }])
        .await
        .unwrap();
        view.insert_bulk([&Kv {
            id: "a".into(),
            val: "replaced".into(),
        }])
        .await
        .unwrap();

        let n: i64 = sqlx::query_scalar("select count(*) from kv_replace")
            .fetch_one(sqlite.pool())
            .await
            .unwrap();
        assert_eq!(n, 1);
        let v: String = sqlx::query_scalar("select val from kv_replace where id='a'")
            .fetch_one(sqlite.pool())
            .await
            .unwrap();
        assert_eq!(v, "replaced");
    }

    #[tokio::test]
    async fn insert_bulk_forces_multiple_chunks() {
        // Single Bind column so `bind_col_count() == 1`: the chunk size (`per`)
        // equals the raw `variable_number_limit`, so we need to insert more than
        // `variable_number_limit` rows to force `insert_bulk_tx`'s `while` loop
        // to run more than once. We read the real, driver-reported limit (never
        // weaken it) and size the row count off of it, so this is deterministic
        // regardless of the sqlite build's actual `SQLITE_LIMIT_VARIABLE_NUMBER`.
        struct OneCol {
            id: String,
        }
        impl Table for OneCol {
            const SCHEMA: Schema = Schema {
                name: "onecol",
                columns: &[Col::key("id")],
                conflict: Conflict::Ignore,
            };
            fn bind_row(
                &self,
                sep: &mut sqlx::query_builder::Separated<'_, SqliteDb, &'static str>,
            ) {
                sep.push_bind(self.id.as_str());
            }
        }
        assert_eq!(OneCol::SCHEMA.bind_col_count(), 1);

        let sqlite = crate::sqlite::Sqlite::builder().memory().open().await.unwrap();
        sqlx::query("create table onecol(id text primary key)")
            .execute(sqlite.pool())
            .await
            .unwrap();

        let limit = sqlite.info().await.variable_number_limit;
        // Comfortably more than 2 full chunks' worth of rows.
        let n_rows = limit * 2 + 137;

        let view = TableView::<OneCol>::new(sqlite.clone());
        let rows: Vec<OneCol> = (0..n_rows)
            .map(|i| OneCol {
                id: format!("id-{i}"),
            })
            .collect();
        view.insert_bulk(rows.iter()).await.unwrap();

        let n: i64 = sqlx::query_scalar("select count(*) from onecol")
            .fetch_one(sqlite.pool())
            .await
            .unwrap();
        assert_eq!(n as usize, n_rows);
    }

    #[tokio::test]
    async fn get_and_delete() {
        struct Kv {
            id: String,
            val: String,
        }
        impl Table for Kv {
            const SCHEMA: Schema = Schema {
                name: "kv2",
                columns: &[Col::key("id"), Col::col("val")],
                conflict: Conflict::Upsert,
            };
            fn bind_row(
                &self,
                sep: &mut sqlx::query_builder::Separated<'_, SqliteDb, &'static str>,
            ) {
                sep.push_bind(self.id.as_str()).push_bind(self.val.as_str());
            }
        }
        impl<'r> sqlx::FromRow<'r, sqlx::sqlite::SqliteRow> for Kv {
            fn from_row(row: &'r sqlx::sqlite::SqliteRow) -> sqlx::Result<Self> {
                use sqlx::Row;
                Ok(Self {
                    id: row.try_get("id")?,
                    val: row.try_get("val")?,
                })
            }
        }

        let sqlite = crate::sqlite::Sqlite::builder().memory().open().await.unwrap();
        sqlx::query("create table kv2(id text primary key, val text)")
            .execute(sqlite.pool())
            .await
            .unwrap();
        let view = TableView::<Kv>::new(sqlite.clone());
        view.insert_one(&Kv {
            id: "a".into(),
            val: "1".into(),
        })
        .await
        .unwrap();

        assert_eq!(view.get("a").await.unwrap().unwrap().val, "1");
        assert!(view.get("zzz").await.unwrap().is_none());
        view.delete("a").await.unwrap();
        assert!(view.get("a").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn all_streams_rows() {
        use futures::TryStreamExt;
        struct Kv {
            id: String,
            val: String,
        }
        impl Table for Kv {
            const SCHEMA: Schema = Schema {
                name: "kv2",
                columns: &[Col::key("id"), Col::col("val")],
                conflict: Conflict::Upsert,
            };
            fn bind_row(
                &self,
                sep: &mut sqlx::query_builder::Separated<'_, SqliteDb, &'static str>,
            ) {
                sep.push_bind(self.id.as_str()).push_bind(self.val.as_str());
            }
        }
        impl<'r> sqlx::FromRow<'r, sqlx::sqlite::SqliteRow> for Kv {
            fn from_row(row: &'r sqlx::sqlite::SqliteRow) -> sqlx::Result<Self> {
                use sqlx::Row;
                Ok(Self {
                    id: row.try_get("id")?,
                    val: row.try_get("val")?,
                })
            }
        }

        let sqlite = crate::sqlite::Sqlite::builder().memory().open().await.unwrap();
        sqlx::query("create table kv2(id text primary key, val text)")
            .execute(sqlite.pool())
            .await
            .unwrap();
        let view = TableView::<Kv>::new(sqlite.clone());
        view.insert_bulk([
            &Kv {
                id: "a".into(),
                val: "1".into(),
            },
            &Kv {
                id: "b".into(),
                val: "2".into(),
            },
        ])
        .await
        .unwrap();

        let rows: Vec<Kv> = view.all().try_collect().await.unwrap();
        assert_eq!(rows.len(), 2);
    }

    #[tokio::test]
    async fn delete_all_removes_everything() {
        struct Kv {
            id: String,
            val: String,
        }
        impl Table for Kv {
            const SCHEMA: Schema = Schema {
                name: "kv_delete_all",
                columns: &[Col::key("id"), Col::col("val")],
                conflict: Conflict::Upsert,
            };
            fn bind_row(
                &self,
                sep: &mut sqlx::query_builder::Separated<'_, SqliteDb, &'static str>,
            ) {
                sep.push_bind(self.id.as_str()).push_bind(self.val.as_str());
            }
        }

        let sqlite = crate::sqlite::Sqlite::builder().memory().open().await.unwrap();
        sqlx::query("create table kv_delete_all(id text primary key, val text)")
            .execute(sqlite.pool())
            .await
            .unwrap();
        let view = TableView::<Kv>::new(sqlite.clone());
        view.insert_bulk([
            &Kv {
                id: "a".into(),
                val: "1".into(),
            },
            &Kv {
                id: "b".into(),
                val: "2".into(),
            },
        ])
        .await
        .unwrap();

        view.delete_all().await.unwrap();

        let n: i64 = sqlx::query_scalar("select count(*) from kv_delete_all")
            .fetch_one(sqlite.pool())
            .await
            .unwrap();
        assert_eq!(n, 0);
    }
}
