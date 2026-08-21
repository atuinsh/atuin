//! Utility crate for defining ORM-like accessors
//!
use std::marker::PhantomData;

use sqlx::query::QueryAs;
use sqlx::sqlite::SqliteArguments;
use sqlx::{Encode, QueryBuilder, Sqlite as SqliteDb, Type};
use tracing::instrument;

use super::Sqlite;

#[doc(hidden)]
pub enum Conflict {
    Ignore,
    Replace,
    Upsert,
}

#[doc(hidden)]
pub enum ColKind {
    Bind,
    Expr(&'static str),
}

#[doc(hidden)]
pub struct Col {
    /// The name of the column.
    pub name: &'static str,
    /// Whether this column is a parimary key.
    ///
    /// If this is set to `true`, the following effects are had:
    ///
    /// - The column will be a conflict target, eg:
    ///
    ///   ```sql
    ///   ON CONFLICT (<all columns marked with key=true>)
    ///   ```
    ///
    /// - The key becomes the search target for the [`TableView::get`] and [`TableView::delete`]
    ///   functions.
    pub key: bool,

    /// Controls whether this column's bindings are to be done as an expression or as a
    /// [`sqlx::bind`] call.
    ///
    /// Within the [`table!`] macro, you can specify either
    ///
    /// ```txt
    /// table!(Kv3 {
    ///     columns: {
    ///         v  => |e| e.v.as_str(),            // Bind
    ///         at => sql("strftime('%s','now')"), // Expr
    ///     },
    /// });
    /// ```
    ///
    /// Using the [`ColKind::Expr`], you can specify an arbitrary SQL expression here.
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

#[doc(hidden)]
pub struct Schema {
    /// The table name.
    pub name: &'static str,
    /// The columns the table contains.
    pub columns: &'static [Col],
    /// What to do on conflict. See [`Conflict`].
    pub conflict: Conflict,
}

impl Schema {
    /// Total number of columns.
    pub const fn col_count(&self) -> usize {
        self.columns.len()
    }

    /// Total number of columns that have variables which can be bound.
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

#[doc(hidden)]
pub trait Table {
    const SCHEMA: Schema;
    fn bind_row(&self, sep: &mut sqlx::query_builder::Separated<'_, SqliteDb, &'static str>);
}

/// Define a new Sqlite table and get some ORM-like accessors.
///
/// This is **not** an ORM facility.
///
/// You can use this macro as follows:
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
/// This will generate a new type [`TableView<Kv3>`] which will give you some useful facilities:
///
///   - [`TableView::get`] will fetch rows given keys.
///   - [`TableView::delete`] will delete rows given keys.
///   - [`TableView::delete_all`] will delete all rows.
///   - [`TableView::insert_bulk`] will insert multiple records.
///   - [`TableView::insert_one`] will insert a record into the table.
#[macro_export]
macro_rules! table {
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

    (@conflict ignore) => { $crate::sqlite::Conflict::Ignore };
    (@conflict replace) => { $crate::sqlite::Conflict::Replace };
    (@conflict upsert) => { $crate::sqlite::Conflict::Upsert };

    (@keys [ $($k:literal),* $(,)? ]) => { &[ $($k),* ] };
    (@keys $k:literal) => { &[ $k ] };

    (@cols_array $key:tt; $($cols:tt)*) => {
        $crate::table!(@cols_acc $key; []; $($cols)*)
    };

    (@cols_acc $key:tt; [$($acc:expr),* $(,)?]; ) => {
        &[ $($acc),* ]
    };
    (@cols_acc $key:tt; [$($acc:expr),* $(,)?]; $cname:ident => sql($sql:literal) $(, $($rest:tt)*)? ) => {
        $crate::table!(@cols_acc $key;
            [ $($acc,)* $crate::sqlite::Col::expr(stringify!($cname), $sql) ];
            $($($rest)*)?
        )
    };
    (@cols_acc $key:tt; [$($acc:expr),* $(,)?]; $cname:ident => | $arg:ident | $body:expr $(, $($rest:tt)*)? ) => {
        $crate::table!(@cols_acc $key;
            [ $($acc,)* $crate::sqlite::Col {
                name: stringify!($cname),
                key: $crate::string::is_one_of(stringify!($cname), $crate::table!(@keys $key)),
                kind: $crate::sqlite::ColKind::Bind,
            } ];
            $($($rest)*)?
        )
    };

    (@bind_each $this:expr, $sep:ident; ) => { };
    (@bind_each $this:expr, $sep:ident; $cname:ident => sql($sql:literal) $(, $($rest:tt)*)? ) => {
        $sep.push($sql);
        $crate::table!(@bind_each $this, $sep; $($($rest)*)?)
    };
    (@bind_each $this:expr, $sep:ident; $cname:ident => | $arg:ident | $body:expr $(, $($rest:tt)*)? ) => {
        $sep.push_bind({ let $arg: &Self = $this; $body });
        $crate::table!(@bind_each $this, $sep; $($($rest)*)?)
    };
}

/// An accessor into a Sqlite table. See [`table!`].
#[derive(Debug, Clone)]
pub struct TableView<T> {
    sqlite: Sqlite,
    _t: PhantomData<T>,
}

impl<T> TableView<T> {
    /// You should not need to directly use this. Please see [`table!`].
    pub fn new(sqlite: Sqlite) -> Self {
        Self {
            sqlite,
            _t: PhantomData,
        }
    }

    /// Grab a handle to the [`Sqlite`] database.
    pub fn sqlite(&self) -> &Sqlite {
        &self.sqlite
    }
}

/// Types that can appear as a single key column value bound via `push_bind`.
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
///
/// Implemented for a single scalar key and for tuples of 2 to 8 columns; the
/// tuple arity must match the number of key columns.
pub trait KeyBind {
    /// Number of leading key columns this binds.
    const ARITY: usize;

    /// Push `col = ?` fragments onto a borrowed builder, for the awaited query
    /// paths (`get`/`delete`/`delete_tx`).
    fn push_where(self, qb: &mut QueryBuilder<SqliteDb>, cols: &[&str]);

    /// Bind the key values, in order, onto an owned query — for streaming reads,
    /// where a borrowed `QueryBuilder` cannot escape into the returned stream.
    fn bind_prefix<'q, O>(
        self,
        query: QueryAs<'q, SqliteDb, O, SqliteArguments>,
    ) -> QueryAs<'q, SqliteDb, O, SqliteArguments>
    where
        Self: 'q;
}

impl<A> KeyBind for A
where
    A: KeyScalar + for<'a> Encode<'a, SqliteDb> + Type<SqliteDb> + Send,
{
    const ARITY: usize = 1;

    fn push_where(self, qb: &mut QueryBuilder<SqliteDb>, cols: &[&str]) {
        qb.push(cols[0]).push(" = ").push_bind(self);
    }

    fn bind_prefix<'q, O>(
        self,
        query: QueryAs<'q, SqliteDb, O, SqliteArguments>,
    ) -> QueryAs<'q, SqliteDb, O, SqliteArguments>
    where
        Self: 'q,
    {
        query.bind(self)
    }
}

// Composite-key impls for tuples of arity 2..=8. Each column pushes
// `<sep>col = ?` and binds its value, with the leading column omitting the
// ` and ` separator.
macro_rules! impl_key_bind_tuple {
    ($( ($t0:ident $i0:tt $(, $t:ident $i:tt)*) ),+ $(,)?) => {
        $(
            impl<$t0 $(, $t)*> KeyBind for ($t0, $($t,)*)
            where
                $t0: for<'a> Encode<'a, SqliteDb> + Type<SqliteDb> + Send,
                $($t: for<'a> Encode<'a, SqliteDb> + Type<SqliteDb> + Send,)*
            {
                const ARITY: usize = [$i0 $(, $i)*].len();

                fn push_where(self, qb: &mut QueryBuilder<SqliteDb>, cols: &[&str]) {
                    qb.push(cols[$i0]).push(" = ").push_bind(self.$i0);
                    $(
                        qb.push(" and ").push(cols[$i]).push(" = ").push_bind(self.$i);
                    )*
                }

                fn bind_prefix<'q, O>(
                    self,
                    query: QueryAs<'q, SqliteDb, O, SqliteArguments>,
                ) -> QueryAs<'q, SqliteDb, O, SqliteArguments>
                where
                    Self: 'q,
                {
                    query.bind(self.$i0) $(.bind(self.$i))*
                }
            }
        )+
    };
}

impl_key_bind_tuple! {
    (A 0, B 1),
    (A 0, B 1, C 2),
    (A 0, B 1, C 2, D 3),
    (A 0, B 1, C 2, D 3, E 4),
    (A 0, B 1, C 2, D 3, E 4, F 5),
    (A 0, B 1, C 2, D 3, E 4, F 5, G 6),
    (A 0, B 1, C 2, D 3, E 4, F 5, G 6, H 7),
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
    /// Insert multiple `items` into the table.
    ///
    /// **Does not commit the transaction.**
    #[instrument(level = "trace", skip_all)]
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

    /// Insert multiple `item`s into the table.
    ///
    /// This function will start a new transaction for you.
    #[instrument(level = "trace", skip_all)]
    pub async fn insert_bulk<'a>(&self, items: impl IntoIterator<Item = &'a T>) -> sqlx::Result<()>
    where
        T: 'a,
    {
        let mut tx = self.sqlite.pool().begin().await?;
        self.insert_bulk_tx(&mut tx, items).await?;
        tx.commit().await
    }

    /// Insert one `item` into the table.
    ///
    /// **If you are trying to insert multiple rows, do NOT use this function.** See
    /// [`Self::insert_bulk`].
    #[instrument(level = "trace", skip_all)]
    pub async fn insert_one(&self, item: &T) -> sqlx::Result<()> {
        self.insert_bulk(std::iter::once(item)).await
    }

    /// Delete rows matching `key`.
    ///
    /// `key` may be a full key (deletes at most one row) or a leading prefix of
    /// it (deletes every row sharing that prefix).
    #[instrument(level = "trace", skip_all)]
    pub async fn delete<K: KeyBind>(&self, key: K) -> sqlx::Result<()> {
        let mut qb =
            sqlx::QueryBuilder::<SqliteDb>::new(format!("delete from {} where ", T::SCHEMA.name));
        key.push_where(&mut qb, &Self::key_cols());
        qb.build().execute(self.sqlite.pool()).await?;
        Ok(())
    }

    /// Drop all rows from this database.
    #[instrument(level = "trace", skip_all)]
    pub async fn delete_all(&self) -> sqlx::Result<()> {
        sqlx::query(sqlx::AssertSqlSafe(format!("delete from {}", T::SCHEMA.name)))
            .execute(self.sqlite.pool())
            .await?;
        Ok(())
    }

    /// Like [`Self::delete_all`], but inside an existing transaction; returns the
    /// number of rows deleted.
    #[instrument(level = "trace", skip_all)]
    pub async fn delete_all_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, SqliteDb>,
    ) -> sqlx::Result<u64> {
        let result = sqlx::query(sqlx::AssertSqlSafe(format!("delete from {}", T::SCHEMA.name)))
            .execute(&mut **tx)
            .await?;
        Ok(result.rows_affected())
    }

    /// Like [`Self::delete`], but runs inside an existing transaction.
    #[instrument(level = "trace", skip_all)]
    pub async fn delete_tx<K: KeyBind>(
        &self,
        tx: &mut sqlx::Transaction<'_, SqliteDb>,
        key: K,
    ) -> sqlx::Result<()> {
        let mut qb =
            sqlx::QueryBuilder::<SqliteDb>::new(format!("delete from {} where ", T::SCHEMA.name));
        key.push_where(&mut qb, &Self::key_cols());
        qb.build().execute(&mut **tx).await?;
        Ok(())
    }

    fn key_cols() -> Vec<&'static str> {
        T::SCHEMA.columns.iter().filter(|c| c.key).map(|c| c.name).collect()
    }
}

impl<T> TableView<T>
where
    T: Table + for<'r> sqlx::FromRow<'r, sqlx::sqlite::SqliteRow> + Send + Unpin,
{
    /// Get the table row given the key.
    #[instrument(level = "trace", skip_all)]
    pub async fn get<K: KeyBind>(&self, key: K) -> sqlx::Result<Option<T>> {
        let mut qb =
            sqlx::QueryBuilder::<SqliteDb>::new(format!("select * from {} where ", T::SCHEMA.name));
        key.push_where(&mut qb, &Self::key_cols());
        qb.build_query_as::<T>().fetch_optional(self.sqlite.pool()).await
    }

    /// Stream all rows.
    ///
    /// If you want a [`Vec`] out of this, call `.try_collect()`.
    pub fn all(&self) -> impl futures::Stream<Item = sqlx::Result<T>> + Send + '_ {
        sqlx::query_as(sqlx::AssertSqlSafe(format!("select * from {}", T::SCHEMA.name)))
            .fetch(self.sqlite.pool())
    }

    /// Stream all rows, ordered by key.
    ///
    /// If you want a [`Vec`], call `.try_collect()`.
    pub fn all_ordered(&self) -> impl futures::Stream<Item = sqlx::Result<T>> + Send + '_ {
        sqlx::query_as::<_, T>(sqlx::AssertSqlSafe(format!(
            "select * from {} order by {}",
            T::SCHEMA.name,
            Self::key_cols().join(", "),
        )))
        .fetch(self.sqlite.pool())
    }

    /// Stream the rows whose leading key column(s) match `prefix`, ordered by key.
    ///
    /// A streaming, key-prefix counterpart to [`Self::get`]: where `get` matches
    /// the full key and returns one row, this matches a leading prefix of it.
    /// `prefix` may be a single value or a tuple of the leading key columns. If
    /// you want a [`Vec`], call `.try_collect()`.
    pub fn filter<'a, K>(
        &'a self,
        prefix: K,
    ) -> impl futures::Stream<Item = sqlx::Result<T>> + Send + 'a
    where
        K: KeyBind + 'a,
    {
        let cols = Self::key_cols();
        let predicate =
            cols[..K::ARITY].iter().map(|c| format!("{c} = ?")).collect::<Vec<_>>().join(" and ");
        let sql = format!(
            "select * from {} where {} order by {}",
            T::SCHEMA.name,
            predicate,
            cols.join(", "),
        );
        prefix
            .bind_prefix(sqlx::query_as::<_, T>(sqlx::AssertSqlSafe(sql)))
            .fetch(self.sqlite.pool())
    }
}

#[cfg(test)]
mod tests {
    use futures::TryStreamExt;
    use rstest::{fixture, rstest};

    use super::*;

    mod macro_hygiene {
        use rstest::rstest;

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

        struct Event {
            host: String,
            id: String,
            body: String,
        }
        crate::table!(Event {
            name: "events",
            key: ["id", "host"],
            conflict: ignore,
            columns: {
                host    => |e| e.host.as_str(),
                id      => |e| e.id.as_str(),
                body    => |e| e.body.as_str(),
                created => sql("strftime('%s','now')"),
            },
        });

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
        fn schema_shape() {
            use crate::sqlite::{ColKind, Conflict, Table};
            assert_eq!(Kv3::SCHEMA.name, "kv3");
            assert_eq!(Kv3::SCHEMA.col_count(), 3);
            assert!(matches!(Kv3::SCHEMA.conflict, Conflict::Upsert));
            assert!(matches!(Single::SCHEMA.conflict, Conflict::Replace));
            assert!(matches!(Event::SCHEMA.conflict, Conflict::Ignore));
            // The Expr column is never a bind param and is never a key.
            assert_eq!(Event::SCHEMA.bind_col_count(), 3);
            assert!(matches!(Event::SCHEMA.columns[3].kind, ColKind::Expr("strftime('%s','now')")));
        }

        #[rstest]
        #[case("host", true)]
        #[case("id", true)]
        #[case("body", false)]
        #[case("created", false)] // an Expr column can never be a key
        fn key_flag_resolved_by_name(#[case] name: &str, #[case] is_key: bool) {
            use crate::sqlite::Table;
            let col = Event::SCHEMA.columns.iter().find(|c| c.name == name).unwrap();
            assert_eq!(col.key, is_key);
        }
    }

    #[rstest]
    #[case(<&str as KeyBind>::ARITY, 1)]
    #[case(<(&str, &str) as KeyBind>::ARITY, 2)]
    #[case(<(i64, i64, i64) as KeyBind>::ARITY, 3)]
    fn keybind_arity(#[case] actual: usize, #[case] expected: usize) {
        assert_eq!(actual, expected);
    }

    #[test]
    fn keybind_binds_only_leading_columns() {
        use sqlx::QueryBuilder;

        let mut qb: QueryBuilder<sqlx::Sqlite> = QueryBuilder::new("");
        "x".push_where(&mut qb, &["a", "b"]);
        assert_eq!(qb.sql(), "a = ?");

        let mut qb: QueryBuilder<sqlx::Sqlite> = QueryBuilder::new("");
        ("x", "y").push_where(&mut qb, &["a", "b"]);
        assert_eq!(qb.sql(), "a = ? and b = ?");
    }

    #[test]
    fn bind_col_count_excludes_expr_columns() {
        struct Toy;
        impl Table for Toy {
            const SCHEMA: Schema = Schema {
                name: "toy",
                columns: &[
                    Col::key("id"),
                    Col::col("body"),
                    Col::expr("at", "strftime('%s','now')"),
                ],
                conflict: Conflict::Upsert,
            };
            fn bind_row(
                &self,
                sep: &mut sqlx::query_builder::Separated<'_, SqliteDb, &'static str>,
            ) {
                sep.push_bind("x").push_bind("y").push("strftime('%s','now')");
            }
        }
        assert_eq!(Toy::SCHEMA.col_count(), 3);
        assert_eq!(Toy::SCHEMA.bind_col_count(), 2);
        assert!(Toy::SCHEMA.columns[0].key);
        assert!(!Toy::SCHEMA.columns[1].key);
    }

    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
    struct Kv {
        id: String,
        val: String,
    }
    impl Kv {
        fn new(id: &str, val: &str) -> Self {
            Self {
                id: id.into(),
                val: val.into(),
            }
        }
    }
    impl Table for Kv {
        const SCHEMA: Schema = Schema {
            name: "kv",
            columns: &[Col::key("id"), Col::col("val")],
            conflict: Conflict::Upsert,
        };
        fn bind_row(&self, sep: &mut sqlx::query_builder::Separated<'_, SqliteDb, &'static str>) {
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

    #[fixture]
    async fn store() -> TableView<Kv> {
        let sqlite = crate::sqlite::Sqlite::builder().memory().open().await.unwrap();
        sqlx::query("create table kv(id text primary key, val text)")
            .execute(sqlite.pool())
            .await
            .unwrap();
        TableView::new(sqlite)
    }

    /// The `id`s currently in the store, sorted (`all()` is unordered).
    async fn ids(store: &TableView<Kv>) -> Vec<String> {
        let mut rows: Vec<Kv> = store.all().try_collect().await.unwrap();
        rows.sort();
        rows.into_iter().map(|r| r.id).collect()
    }

    #[rstest]
    #[tokio::test]
    async fn get_insert_delete(#[future(awt)] store: TableView<Kv>) {
        // Empty table, and deleting a missing key, are both no-ops (not errors).
        assert!(store.get("a").await.unwrap().is_none());
        store.delete("a").await.unwrap();

        store.insert_one(&Kv::new("a", "1")).await.unwrap();
        assert_eq!(store.get("a").await.unwrap().unwrap(), Kv::new("a", "1"));

        // Deleting an unrelated key leaves the row intact.
        store.delete("b").await.unwrap();
        assert!(store.get("a").await.unwrap().is_some());

        store.delete("a").await.unwrap();
        assert!(store.get("a").await.unwrap().is_none());
    }

    #[rstest]
    #[tokio::test]
    async fn insert_bulk_empty_is_noop(#[future(awt)] store: TableView<Kv>) {
        store.insert_bulk(std::iter::empty::<&Kv>()).await.unwrap();
        assert!(ids(&store).await.is_empty());
    }

    #[rstest]
    #[tokio::test]
    async fn all_and_delete_all(#[future(awt)] store: TableView<Kv>) {
        assert!(ids(&store).await.is_empty());
        store.insert_bulk([&Kv::new("b", "2"), &Kv::new("a", "1")]).await.unwrap();
        assert_eq!(ids(&store).await, ["a", "b"].map(String::from));
        store.delete_all().await.unwrap();
        assert!(ids(&store).await.is_empty());
    }

    #[rstest]
    #[tokio::test]
    async fn delete_all_tx_reports_count(#[future(awt)] store: TableView<Kv>) {
        store.insert_bulk([&Kv::new("a", "1"), &Kv::new("b", "2")]).await.unwrap();
        let mut tx = store.sqlite().pool().begin().await.unwrap();
        let deleted = store.delete_all_tx(&mut tx).await.unwrap();
        tx.commit().await.unwrap();
        assert_eq!(deleted, 2);
        assert!(ids(&store).await.is_empty());
    }

    #[rstest]
    #[tokio::test]
    async fn insert_bulk_spans_multiple_chunks(#[future(awt)] store: TableView<Kv>) {
        // Size the row count off the real, driver-reported bind-variable limit
        // (never weaken it) so `insert_bulk_tx`'s `while` loop is forced to run
        // several times regardless of the sqlite build.
        let limit = store.sqlite().info().await.variable_number_limit;
        let n = limit * 2 + 137;
        let rows: Vec<Kv> = (0..n).map(|i| Kv::new(&format!("id-{i}"), "v")).collect();
        store.insert_bulk(rows.iter()).await.unwrap();
        assert_eq!(ids(&store).await.len(), n);
    }

    macro_rules! conflict_test {
        ($test:ident, $ty:ident, $table:literal, $conflict:ident, $final_a:literal) => {
            #[tokio::test]
            async fn $test() {
                struct $ty {
                    id: String,
                    val: String,
                }
                impl Table for $ty {
                    const SCHEMA: Schema = Schema {
                        name: $table,
                        columns: &[Col::key("id"), Col::col("val")],
                        conflict: Conflict::$conflict,
                    };
                    fn bind_row(
                        &self,
                        sep: &mut sqlx::query_builder::Separated<'_, SqliteDb, &'static str>,
                    ) {
                        sep.push_bind(self.id.as_str()).push_bind(self.val.as_str());
                    }
                }
                let row = |id: &str, val: &str| $ty {
                    id: id.into(),
                    val: val.into(),
                };

                let sqlite = crate::sqlite::Sqlite::builder().memory().open().await.unwrap();
                sqlx::query(concat!("create table ", $table, "(id text primary key, val text)"))
                    .execute(sqlite.pool())
                    .await
                    .unwrap();
                let view = TableView::<$ty>::new(sqlite);

                view.insert_one(&row("a", "first")).await.unwrap();
                view.insert_bulk([&row("a", "second"), &row("b", "x")]).await.unwrap();

                let count: i64 = sqlx::query_scalar(concat!("select count(*) from ", $table))
                    .fetch_one(view.sqlite().pool())
                    .await
                    .unwrap();
                assert_eq!(count, 2, "`b` lands regardless of the conflict on `a`");

                let a: String =
                    sqlx::query_scalar(concat!("select val from ", $table, " where id = 'a'"))
                        .fetch_one(view.sqlite().pool())
                        .await
                        .unwrap();
                assert_eq!(a, $final_a);
            }
        };
    }
    conflict_test!(upsert_overwrites, KvUpsert, "c_upsert", Upsert, "second");
    conflict_test!(ignore_keeps_first, KvIgnore, "c_ignore", Ignore, "first");
    conflict_test!(replace_overwrites, KvReplace, "c_replace", Replace, "second");

    #[tokio::test]
    async fn filter_ordered_and_prefix_delete() {
        struct Ev {
            host: String,
            id: String,
            val: String,
        }
        impl Table for Ev {
            const SCHEMA: Schema = Schema {
                name: "ev",
                columns: &[Col::key("host"), Col::key("id"), Col::col("val")],
                conflict: Conflict::Upsert,
            };
            fn bind_row(
                &self,
                sep: &mut sqlx::query_builder::Separated<'_, SqliteDb, &'static str>,
            ) {
                sep.push_bind(self.host.as_str())
                    .push_bind(self.id.as_str())
                    .push_bind(self.val.as_str());
            }
        }
        impl<'r> sqlx::FromRow<'r, sqlx::sqlite::SqliteRow> for Ev {
            fn from_row(row: &'r sqlx::sqlite::SqliteRow) -> sqlx::Result<Self> {
                use sqlx::Row;
                Ok(Self {
                    host: row.try_get("host")?,
                    id: row.try_get("id")?,
                    val: row.try_get("val")?,
                })
            }
        }
        let row = |host: &str, id: &str, val: &str| Ev {
            host: host.into(),
            id: id.into(),
            val: val.into(),
        };

        let sqlite = crate::sqlite::Sqlite::builder().memory().open().await.unwrap();
        sqlx::query("create table ev(host text, id text, val text, primary key(host, id))")
            .execute(sqlite.pool())
            .await
            .unwrap();
        let view = TableView::<Ev>::new(sqlite);

        // Empty table: both streams are empty.
        assert!(view.all_ordered().try_collect::<Vec<Ev>>().await.unwrap().is_empty());
        assert!(view.filter("h1").try_collect::<Vec<Ev>>().await.unwrap().is_empty());

        // Insert out of key order to prove the ordering comes from the query.
        view.insert_bulk([&row("h1", "b", "1"), &row("h1", "a", "2"), &row("h2", "a", "3")])
            .await
            .unwrap();

        let all: Vec<(String, String)> =
            view.all_ordered().map_ok(|e| (e.host, e.id)).try_collect().await.unwrap();
        assert_eq!(
            all,
            [("h1", "a"), ("h1", "b"), ("h2", "a")].map(|(h, i)| (h.to_string(), i.to_string()))
        );

        // Scalar prefix (leading key column), ordered by key.
        let h1: Vec<String> = view.filter("h1").map_ok(|e| e.id).try_collect().await.unwrap();
        assert_eq!(h1, ["a", "b"].map(String::from));

        // A prefix that matches nothing yields an empty stream.
        assert!(view.filter("nope").try_collect::<Vec<Ev>>().await.unwrap().is_empty());

        // Full 2-column key (tuple prefix) selects the single row.
        let one: Vec<String> =
            view.filter(("h1", "a")).map_ok(|e| e.val).try_collect().await.unwrap();
        assert_eq!(one, ["2".to_string()]);

        // `delete` with a scalar prefix removes every `h1` row.
        view.delete("h1").await.unwrap();
        let hosts: Vec<String> = view.all_ordered().map_ok(|e| e.host).try_collect().await.unwrap();
        assert_eq!(hosts, ["h2".to_string()]);

        // `delete_tx` with the full key, committed in a transaction.
        let mut tx = view.sqlite().pool().begin().await.unwrap();
        view.delete_tx(&mut tx, ("h2", "a")).await.unwrap();
        tx.commit().await.unwrap();
        assert!(view.all_ordered().try_collect::<Vec<Ev>>().await.unwrap().is_empty());
    }
}
