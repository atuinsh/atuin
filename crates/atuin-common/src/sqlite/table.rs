//! Utility crate for defining ORM-like accessors
//!
//! This is **not** an ORM facility.
//!
//! ## Guide
//!
//! Assuming you have some structure
//!
//! ```ignore
//! #[derive(sqlx::FromRow)]
//! struct Kv3 {
//!   ns: String,
//!   k: u32,
//!   v: String
//! }
//! ```
//!
//! You can define a [`TableView`] into a SQL table which will create convenient operations on that
//! table. The type you have defined must implement [`sqlx::FromRow`] and is considered to be the
//! Rust representation of the row.
//!
//! To define this view:
//!
//! ```ignore
//! table!(Kv3 {
//!     // The name of the table in SQLite.
//!     name: "kv3",
//!     // The keys to use when getting/updating/deleting.
//!     //
//!     // The macro will generate accessors which will require you provide these keys for
//!     // `delete`, `update`, `get`.
//!     //
//!     // You can also specify this as `key: "<your-key>"`.
//!     key: ["ns", "k"],
//!     // The conflict behavior on insert. This is required and can be either `ignore`, `replace`,
//!     // or `upsert`.
//!     conflict: upsert,
//!     // Defines the columns of this table. You **must** always list out the columns. The columns
//!     // map to their "binding generators", ie. thin functions which provide SQL-compatible types
//!     // from the types in your struct.
//!     columns: {
//!         ns => |e| e.ns.as_str(),
//!         k  => |e| e.k,
//!         v  => |e| e.v.as_str(),
//!         // Inserts a plain SQL string for this column. You are given a reference to the object
//!         // and can craft your bindings however you want.
//!         at => raw(|_| "strftime('%s','now')"),
//!     },
//! });
//! ```
//!
//! This will generate a new type [`TableView<Kv3>`] which will give you some useful facilities:
//!
//!   - [`TableView::insert_one`] will insert a single record into the table.
//!   - [`TableView::insert_bulk`] will insert multiple records in a new transaction.
//!   - [`TableView::get`] will fetch a single row given its full key.
//!   - [`TableView::all`] streams all rows.
//!   - [`TableView::all_ordered`] streams all rows, ordered by key.
//!   - [`TableView::filter_by`] streams rows where a column equals a value, ordered by key.
//!   - [`TableView::count`] returns the number of rows in the table.
//!   - [`TableView::update_one`] will update a single record.
//!   - [`TableView::delete`] will delete the single row with a given full key.
//!   - [`TableView::delete_many`] will delete rows matching any of several keys.
//!   - [`TableView::delete_all`] will delete all rows.
//!
//! To run writes inside a caller-owned transaction (optionally spanning several
//! tables), call [`TableView::on`] to get a [`TxView`] and use its
//! [`insert_bulk`](TxView::insert_bulk), [`update_one`](TxView::update_one),
//! [`delete`](TxView::delete), and [`delete_all`](TxView::delete_all) methods.
//!
//! in the case of the given example, the rough SQL statement that would be generated for
//! [`TableView::get`] is something like:
//!
//! ```sql
//! SELECT ns, k, v FROM kv3 WHERE ns = ? AND k = ?;
//! ```
//!
//! Instantiating this structure can now be done like so:
//!
//! ```ignore
//! let sqlite = Sqlite::new();
//! let table = TableView::<Kv3>::new(sqlite);
//! ```
//!
//! For more info on the `Sqlite` type, see [`super::Sqlite`]. You can now run operations on this
//! table:
//!
//! ```ignore
//! let my_kv3: Option<Kv3> = table.get(("my_ns", 3)).await?;
//! ```
//!
//! ### Complete Example
//!
//! ```
//! use atuin_common::sqlite::{Sqlite, TableView};
//! use atuin_common::table;
//!
//! #[derive(sqlx::FromRow)]
//! struct Kv3 {
//!     ns: String,
//!     k: u32,
//!     v: String,
//! }
//!
//! table!(Kv3 {
//!     name: "kv3",
//!     key: ["ns", "k"],
//!     conflict: upsert,
//!     columns: {
//!         ns => |e| e.ns.as_str(),
//!         k  => |e| e.k,
//!         v  => |e| e.v.as_str(),
//!     },
//! });
//!
//! #[tokio::main]
//! async fn main() {
//!     let sqlite = Sqlite::builder().memory().open().await.unwrap();
//!
//!     // `table!` does not create the table - your migrations do. We create it inline
//!     // here so the example is self-contained; the columns must match the schema above.
//!     sqlx::query(
//!         "CREATE TABLE kv3 (ns TEXT NOT NULL, k INTEGER NOT NULL, v TEXT NOT NULL, PRIMARY KEY (ns, k))",
//!     )
//!     .execute(sqlite.pool())
//!     .await
//!     .unwrap();
//!
//!     let table = TableView::<Kv3>::new(sqlite);
//!
//!     // insert a single row, then a batch (in one transaction).
//!     table.insert_one(&Kv3 { ns: "app".into(), k: 1, v: "hello".into() }).await.unwrap();
//!     table.insert_bulk([
//!         &Kv3 { ns: "app".into(), k: 2, v: "world".into() },
//!         &Kv3 { ns: "app".into(), k: 3, v: "!".into() },
//!     ]).await.unwrap();
//!
//!     // Fetch a single row by its composite key.
//!     let row: Option<Kv3> = table.get(("app", 1)).await.unwrap();
//!     assert_eq!(row.unwrap().v, "hello");
//!
//!     // update a row (matched on its key), then count the table.
//!     table.update_one(&Kv3 { ns: "app".into(), k: 1, v: "hi".into() }).await.unwrap();
//!     assert_eq!(table.count().await.unwrap(), 3);
//!
//!     // delete a single row, then everything.
//!     table.delete(("app", 3)).await.unwrap();
//!     table.delete_all().await.unwrap();
//! }
//! ```
use std::borrow::Cow;
use std::marker::PhantomData;

use sqlx::query::{Query, QueryAs};
use sqlx::sqlite::SqliteArguments;
use sqlx::{Encode, QueryBuilder, Sqlite as SqliteDb, Transaction, Type};
use tracing::instrument;

use super::Sqlite;

#[doc(hidden)]
#[derive(Clone, Copy)]
pub enum Conflict {
    Ignore,
    Replace,
    Upsert,
}

#[doc(hidden)]
pub enum ColKind {
    Bind,
    Expr,
}

/// Represents a statically defined column registered within a [`Schema`].
///
/// You should **not** use this directly. See the [`table!`] macro.
pub struct Col {
    /// The name of the column.
    pub name: &'static str,
    /// Whether this column is a primary key.
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

    /// Controls whether this column's value is bound as a `?` parameter (via
    /// `push_bind`) or emitted as literal SQL.
    ///
    /// Within the [`table!`] macro, you can specify either
    ///
    /// ```txt
    /// table!(Kv3 {
    ///     columns: {
    ///         v  => |e| e.v.as_str(),                // Bind
    ///         at => raw(|_| "strftime('%s','now')"), // Expr
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

    pub const fn expr(name: &'static str) -> Self {
        Self {
            name,
            key: false,
            kind: ColKind::Expr,
        }
    }
}

/// Defines the table schema.
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

/// Table trait, internal to this crate, which is implemented by tables generated by the [`table!`]
/// macro.
///
/// You should **not** implement this yourself.
pub trait Table {
    const SCHEMA: Schema;

    /// The key columns, in the order they appear in `columns` (which the
    /// [`table!`] macro enforces is also their declared order). This is the
    /// order a key tuple's values bind in.
    const KEY_COLS: &'static [&'static str];

    /// The table's columns as a comma-separated SQL list (`"ns, k, v"`), in
    /// column order. Used to build explicit `SELECT` lists.
    const SQL_COLS: &'static str;
    /// The key columns as a comma-separated SQL list (`"ns, k"`), in column
    /// order. Used for `ORDER BY`.
    const SQL_KEYS_CSV: &'static str;

    const SQL_SELECT_ALL: &'static str;
    const SQL_SELECT_ALL_ORDERED: &'static str;
    const SQL_COUNT: &'static str;
    const SQL_DELETE_ALL: &'static str;
    const SQL_INSERT_PREFIX: &'static str;
    /// The clause appended after an insert's `VALUES`: the `ON CONFLICT ... DO
    /// UPDATE SET ...` upsert clause, or `""` for `ignore`/`replace` (whose
    /// conflict behavior is carried by the insert verb in
    /// [`SQL_INSERT_PREFIX`](Self::SQL_INSERT_PREFIX)). Always safe to append.
    const SQL_INSERT_SUFFIX: &'static str;
    const SQL_UPDATE: &'static str;
    /// The full-key predicate `key0 = ? AND key1 = ?`, in column order. Shared by the statements
    /// that match a single row by key.
    const SQL_WHERE_KEY: &'static str;
    /// `SELECT <cols> FROM <table> WHERE <key predicate>` - the whole of [`TableView::get`], with
    /// nothing left to build at runtime.
    const SQL_GET: &'static str;
    /// `DELETE FROM <table> WHERE <key predicate>` - the whole of [`TableView::delete`].
    const SQL_DELETE_BY_KEY: &'static str;

    /// Append this row's column values to `sep`, in column order, for an insert: every `Bind`
    /// column pushes a `?` parameter bound to its value; every `Expr` column pushes its literal SQL
    /// (binding nothing). Generated by [`table!`].
    fn bind_row(&self, sep: &mut sqlx::query_builder::Separated<'_, SqliteDb, &'static str>);

    /// Bind the values for an `UPDATE ... SET ... WHERE <key>`: every non-key `Bind` column's value
    /// (the SET list), then the key values (the WHERE clause), in column order. Generated by
    /// [`table!`].
    fn bind_update<'q>(
        &'q self,
        query: Query<'q, SqliteDb, SqliteArguments>,
    ) -> Query<'q, SqliteDb, SqliteArguments>;
}

/// Define a new Sqlite table and get some ORM-like accessors.
///
/// This is **not** an ORM facility. See the module-level documentation for a full guide and
/// example.
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

            const KEY_COLS: &'static [&'static str] = $crate::table!(@keys $key);

            const SQL_COLS: &'static str = $crate::string::strip_tail(
                $crate::table!(@names_csv $($cols)*),
                $crate::sqlite::SEP_COMMA.len(),
            );
            const SQL_KEYS_CSV: &'static str = $crate::string::strip_tail(
                $crate::table!(@keys_csv $key; $($cols)*),
                $crate::sqlite::SEP_COMMA.len(),
            );

            const SQL_SELECT_ALL: &'static str = $crate::sqlite::concatcp!(
                "SELECT ",
                <$ty as $crate::sqlite::Table>::SQL_COLS,
                " FROM ",
                $name,
            );
            const SQL_SELECT_ALL_ORDERED: &'static str = $crate::sqlite::concatcp!(
                <$ty as $crate::sqlite::Table>::SQL_SELECT_ALL,
                " ORDER BY ",
                <$ty as $crate::sqlite::Table>::SQL_KEYS_CSV,
            );
            const SQL_COUNT: &'static str =
                $crate::sqlite::concatcp!("SELECT COUNT(*) FROM ", $name);
            const SQL_DELETE_ALL: &'static str =
                $crate::sqlite::concatcp!("DELETE FROM ", $name);
            const SQL_INSERT_PREFIX: &'static str = $crate::sqlite::concatcp!(
                $crate::sqlite::insert_verb($crate::table!(@conflict $conflict)),
                $name,
                "(",
                <$ty as $crate::sqlite::Table>::SQL_COLS,
                ") ",
            );
            const SQL_INSERT_SUFFIX: &'static str =
                $crate::table!(@insert_suffix $conflict; $key; $($cols)*);
            const SQL_WHERE_KEY: &'static str = $crate::string::strip_tail(
                $crate::table!(@update_where_csv $key; $($cols)*),
                $crate::sqlite::SEP_AND.len(),
            );
            const SQL_UPDATE: &'static str = {
                const SETS: &str = $crate::string::strip_tail(
                    $crate::table!(@update_sets_csv $key; $($cols)*),
                    $crate::sqlite::SEP_COMMA.len(),
                );
                $crate::sqlite::concatcp!(
                    "UPDATE ", $name, " SET ", SETS,
                    " WHERE ", <$ty as $crate::sqlite::Table>::SQL_WHERE_KEY,
                )
            };
            const SQL_GET: &'static str = $crate::sqlite::concatcp!(
                "SELECT ",
                <$ty as $crate::sqlite::Table>::SQL_COLS,
                " FROM ",
                $name,
                " WHERE ",
                <$ty as $crate::sqlite::Table>::SQL_WHERE_KEY,
            );
            const SQL_DELETE_BY_KEY: &'static str = $crate::sqlite::concatcp!(
                "DELETE FROM ",
                $name,
                " WHERE ",
                <$ty as $crate::sqlite::Table>::SQL_WHERE_KEY,
            );

            fn bind_row(
                &self,
                sep: &mut sqlx::query_builder::Separated<'_, sqlx::Sqlite, &'static str>,
            ) {
                $crate::table!(@bind_each self, sep; $($cols)*);
            }

            fn bind_update<'q>(
                &'q self,
                query: sqlx::query::Query<'q, sqlx::Sqlite, sqlx::sqlite::SqliteArguments>,
            ) -> sqlx::query::Query<'q, sqlx::Sqlite, sqlx::sqlite::SqliteArguments> {
                let mut query = query;
                // SET: non-key columns, in column order (Expr columns are literal
                // SQL, so they bind nothing).
                $crate::table!(@bind_update_set self, query, $key; $($cols)*);
                // WHERE: key columns, in column order.
                $crate::table!(@bind_update_where self, query, $key; $($cols)*);
                query
            }
        }

        #[allow(non_upper_case_globals, dead_code)]
        impl $ty {
            $crate::table!(@col_consts $ty; $($cols)*);
        }

        const _: () = ::core::assert!(
            $crate::sqlite::keys_in_column_order(
                &<$ty as $crate::sqlite::Table>::SCHEMA,
                <$ty as $crate::sqlite::Table>::KEY_COLS,
            ),
            "table!: `key` columns must be listed in the order they appear in `columns`",
        );

        const _: () = ::core::assert!(
            $crate::sqlite::is_sql_ident($name),
            "table!: `name` must be a bare SQL identifier (it is spliced into SQL unquoted)",
        );
    };

    (@conflict ignore) => { $crate::sqlite::Conflict::Ignore };
    (@conflict replace) => { $crate::sqlite::Conflict::Replace };
    (@conflict upsert) => { $crate::sqlite::Conflict::Upsert };

    (@insert_suffix upsert; $key:tt; $($cols:tt)*) => {{
        const KEYS: &str = $crate::string::strip_tail(
            $crate::table!(@keys_csv $key; $($cols)*),
            $crate::sqlite::SEP_COMMA.len(),
        );
        const SETS: &str = $crate::string::strip_tail(
            $crate::table!(@upsert_sets_csv $key; $($cols)*),
            $crate::sqlite::SEP_COMMA.len(),
        );
        const _: () = ::core::assert!(
            !SETS.is_empty(),
            "table!: `conflict: upsert` needs at least one non-key column to update on conflict",
        );
        $crate::sqlite::concatcp!(" ON CONFLICT (", KEYS, ") DO UPDATE SET ", SETS)
    }};
    (@insert_suffix ignore; $key:tt; $($cols:tt)*) => { "" };
    (@insert_suffix replace; $key:tt; $($cols:tt)*) => { "" };

    (@keys [ $($k:literal),* $(,)? ]) => { &[ $($k),* ] };
    (@keys $k:literal) => { &[ $k ] };

    (@names_csv ) => { "" };
    (@names_csv $cname:ident => raw(|$a:tt| $b:expr) $(, $($rest:tt)*)? ) => {
        $crate::sqlite::concatcp!(
            stringify!($cname), $crate::sqlite::SEP_COMMA, $crate::table!(@names_csv $($($rest)*)?)
        )
    };
    (@names_csv $cname:ident => | $a:ident | $b:expr $(, $($rest:tt)*)? ) => {
        $crate::sqlite::concatcp!(
            stringify!($cname), $crate::sqlite::SEP_COMMA, $crate::table!(@names_csv $($($rest)*)?)
        )
    };

    (@keys_csv $key:tt; ) => { "" };
    (@keys_csv $key:tt; $cname:ident => $(raw(|$a:tt| $b:expr))? $(| $ba:ident | $bb:expr)? $(, $($rest:tt)*)? ) => {
        $crate::sqlite::concatcp!(
            $crate::sqlite::keep(
                $crate::string::is_one_of(stringify!($cname), $crate::table!(@keys $key)),
                $crate::sqlite::concatcp!(stringify!($cname), $crate::sqlite::SEP_COMMA),
            ),
            $crate::table!(@keys_csv $key; $($($rest)*)?),
        )
    };

    (@upsert_sets_csv $key:tt; ) => { "" };
    (@upsert_sets_csv $key:tt; $cname:ident => $(raw(|$a:tt| $b:expr))? $(| $ba:ident | $bb:expr)? $(, $($rest:tt)*)? ) => {
        $crate::sqlite::concatcp!(
            $crate::sqlite::keep(
                !$crate::string::is_one_of(stringify!($cname), $crate::table!(@keys $key)),
                $crate::sqlite::concatcp!(
                    stringify!($cname), " = EXCLUDED.", stringify!($cname), $crate::sqlite::SEP_COMMA
                ),
            ),
            $crate::table!(@upsert_sets_csv $key; $($($rest)*)?),
        )
    };

    (@update_sets_csv $key:tt; ) => { "" };
    (@update_sets_csv $key:tt; $cname:ident => raw(|$a:tt| $b:expr) $(, $($rest:tt)*)? ) => {
        $crate::table!(@update_sets_csv $key; $($($rest)*)?)
    };
    (@update_sets_csv $key:tt; $cname:ident => | $a:ident | $b:expr $(, $($rest:tt)*)? ) => {
        $crate::sqlite::concatcp!(
            $crate::sqlite::keep(
                !$crate::string::is_one_of(stringify!($cname), $crate::table!(@keys $key)),
                $crate::sqlite::concatcp!(stringify!($cname), " = ?", $crate::sqlite::SEP_COMMA),
            ),
            $crate::table!(@update_sets_csv $key; $($($rest)*)?),
        )
    };

    (@update_where_csv $key:tt; ) => { "" };
    (@update_where_csv $key:tt; $cname:ident => $(raw(|$a:tt| $b:expr))? $(| $ba:ident | $bb:expr)? $(, $($rest:tt)*)? ) => {
        $crate::sqlite::concatcp!(
            $crate::sqlite::keep(
                $crate::string::is_one_of(stringify!($cname), $crate::table!(@keys $key)),
                $crate::sqlite::concatcp!(stringify!($cname), " = ?", $crate::sqlite::SEP_AND),
            ),
            $crate::table!(@update_where_csv $key; $($($rest)*)?),
        )
    };

    (@col_consts $ty:ty; ) => {};
    (@col_consts $ty:ty; $cname:ident => raw(|$arg:tt| $body:expr) $(, $($rest:tt)*)? ) => {
        pub const $cname: $crate::sqlite::ColRef<$ty> =
            $crate::sqlite::ColRef::new(stringify!($cname));
        $crate::table!(@col_consts $ty; $($($rest)*)?);
    };
    (@col_consts $ty:ty; $cname:ident => | $arg:ident | $body:expr $(, $($rest:tt)*)? ) => {
        pub const $cname: $crate::sqlite::ColRef<$ty> =
            $crate::sqlite::ColRef::new(stringify!($cname));
        $crate::table!(@col_consts $ty; $($($rest)*)?);
    };

    (@cols_array $key:tt; $($cols:tt)*) => {
        $crate::table!(@cols_acc $key; []; $($cols)*)
    };

    (@cols_acc $key:tt; [$($acc:expr),* $(,)?]; ) => {
        &[ $($acc),* ]
    };
    (@cols_acc $key:tt; [$($acc:expr),* $(,)?]; $cname:ident => raw(|$arg:tt| $body:expr) $(, $($rest:tt)*)? ) => {
        $crate::table!(@cols_acc $key;
            [ $($acc,)* $crate::sqlite::Col::expr(stringify!($cname)) ];
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
    (@bind_each $this:expr, $sep:ident; $cname:ident => raw(|$arg:tt| $body:expr) $(, $($rest:tt)*)? ) => {
        $sep.push({ let $arg: &Self = $this; $body });
        $crate::table!(@bind_each $this, $sep; $($($rest)*)?)
    };
    (@bind_each $this:expr, $sep:ident; $cname:ident => | $arg:ident | $body:expr $(, $($rest:tt)*)? ) => {
        $sep.push_bind({ let $arg: &Self = $this; $body });
        $crate::table!(@bind_each $this, $sep; $($($rest)*)?)
    };

    (@bind_update_set $this:expr, $q:ident, $key:tt; ) => { };
    (@bind_update_set $this:expr, $q:ident, $key:tt; $cname:ident => raw(|$arg:tt| $body:expr) $(, $($rest:tt)*)? ) => {
        $crate::table!(@bind_update_set $this, $q, $key; $($($rest)*)?)
    };
    (@bind_update_set $this:expr, $q:ident, $key:tt; $cname:ident => | $arg:ident | $body:expr $(, $($rest:tt)*)? ) => {
        if !$crate::string::is_one_of(stringify!($cname), $crate::table!(@keys $key)) {
            $q = $q.bind({ let $arg: &Self = $this; $body });
        }
        $crate::table!(@bind_update_set $this, $q, $key; $($($rest)*)?)
    };

    (@bind_update_where $this:expr, $q:ident, $key:tt; ) => { };
    (@bind_update_where $this:expr, $q:ident, $key:tt; $cname:ident => raw(|$arg:tt| $body:expr) $(, $($rest:tt)*)? ) => {
        $crate::table!(@bind_update_where $this, $q, $key; $($($rest)*)?)
    };
    (@bind_update_where $this:expr, $q:ident, $key:tt; $cname:ident => | $arg:ident | $body:expr $(, $($rest:tt)*)? ) => {
        if $crate::string::is_one_of(stringify!($cname), $crate::table!(@keys $key)) {
            $q = $q.bind({ let $arg: &Self = $this; $body });
        }
        $crate::table!(@bind_update_where $this, $q, $key; $($($rest)*)?)
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

/// A type-safe reference to one column of table `T`. The `table!` macro emits one per column as an
/// associated const on the row type (e.g. `Kv3::ns`).
///
/// You should not implement this yourself.
pub struct ColRef<T> {
    name: &'static str,
    _t: PhantomData<T>,
}

// Hand-written, not `#[derive]`: derived `Copy`/`Clone` would bound `T: Copy`, leaving
// `ColRef<RowType>` non-`Copy` for the (non-`Copy`) row types it's used with. `PhantomData<T>`
// needs no such bound.
impl<T> Clone for ColRef<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> Copy for ColRef<T> {}

impl<T> ColRef<T> {
    /// Construct a column reference. The [`table!`] macro calls this with each column's
    /// `stringify!`d identifier; you should use the generated `ColRef` consts (e.g. `Kv3::ns`)
    /// rather than this directly. `name` must be a bare SQL identifier that names a column of `T` -
    /// both are checked at compile time, since it is spliced into SQL unquoted.
    #[doc(hidden)]
    pub const fn new(name: &'static str) -> Self
    where
        T: Table,
    {
        assert!(is_sql_ident(name), "ColRef name must be a bare SQL identifier");
        assert!(has_column(&T::SCHEMA, name), "ColRef names a column that is not in the table");
        Self {
            name,
            _t: PhantomData,
        }
    }

    /// The column's SQL name.
    pub const fn name(self) -> &'static str {
        self.name
    }
}

/// A sqlx query that accepts positional value binds - implemented for both
/// [`Query`] (statements run with `execute`) and [`QueryAs`] (statements run
/// with `fetch`), so [`KeyBind::bind_key`] can fill the `?` placeholders of a
/// precomputed statement regardless of which it is.
///
/// Do **not** use this directly.
#[doc(hidden)]
pub trait BindQuery<'q>: Sized {
    fn bind_value<V>(self, value: V) -> Self
    where
        V: Encode<'q, SqliteDb> + Type<SqliteDb> + Send + 'q;
}

impl<'q> BindQuery<'q> for Query<'q, SqliteDb, SqliteArguments> {
    fn bind_value<V>(self, value: V) -> Self
    where
        V: Encode<'q, SqliteDb> + Type<SqliteDb> + Send + 'q,
    {
        self.bind(value)
    }
}

impl<'q, O> BindQuery<'q> for QueryAs<'q, SqliteDb, O, SqliteArguments> {
    fn bind_value<V>(self, value: V) -> Self
    where
        V: Encode<'q, SqliteDb> + Type<SqliteDb> + Send + 'q,
    {
        self.bind(value)
    }
}

/// Binds a table's key onto a query and builds the SQL fragments that reference it.
///
/// Implemented for a single scalar key and for tuples of 2 to 8 columns; the tuple arity must match
/// the number of key columns.
pub trait KeyBind {
    /// Number of key columns this binds.
    ///
    /// ```
    /// # use atuin_common::sqlite::KeyBind;
    /// assert_eq!(<&str as KeyBind>::ARITY, 1);
    /// assert_eq!(<(&str, &str, &str) as KeyBind>::ARITY, 3);
    /// ```
    const ARITY: usize;

    /// Bind the key values, in column order, onto a query whose `?` placeholders were written by a
    /// precomputed statement (e.g. `Table::SQL_GET`). Writes no SQL of its own.
    fn bind_key<'q, Q: BindQuery<'q>>(self, query: Q) -> Q
    where
        Self: 'q;

    /// Push this key as one element of an `in` value list: `?` for a scalar key, `(?, ...)` for a
    /// composite key. Paired with [`KeyBind::in_lhs`], this builds `lhs IN (row, row, ...)`.
    ///
    /// ```
    /// # use atuin_common::sqlite::KeyBind;
    /// # use sqlx::QueryBuilder;
    /// // A scalar is a bare placeholder.
    /// # let mut qb = QueryBuilder::<sqlx::Sqlite>::new("");
    /// "x".push_row(&mut qb);
    /// assert_eq!(qb.sql(), "?");
    ///
    /// // A tuple is a parenthesised row.
    /// # let mut qb = QueryBuilder::<sqlx::Sqlite>::new("");
    /// ("x", "y").push_row(&mut qb);
    /// assert_eq!(qb.sql(), "(?, ?)");
    /// ```
    fn push_row(self, qb: &mut QueryBuilder<SqliteDb>);

    /// The left-hand side of an `in` test over these key columns: `col` for a scalar key, `(col,
    /// ...)` for a composite key.
    ///
    /// The scalar case borrows the column name; only the composite case allocates.
    ///
    /// ```
    /// # use atuin_common::sqlite::KeyBind;
    /// assert_eq!(&*<&str as KeyBind>::in_lhs(&["id"]), "id");
    /// assert_eq!(&*<(&str, &str) as KeyBind>::in_lhs(&["host", "id"]), "(host, id)");
    /// ```
    fn in_lhs<'c>(cols: &[&'c str]) -> Cow<'c, str> {
        if Self::ARITY == 1 {
            return Cow::Borrowed(cols[0]);
        }
        let cols = &cols[..Self::ARITY];
        let len = 2 + cols.iter().map(|c| c.len()).sum::<usize>() + 2 * (cols.len() - 1);
        let mut s = String::with_capacity(len);
        s.push('(');
        for (i, c) in cols.iter().enumerate() {
            if i > 0 {
                s.push_str(", ");
            }
            s.push_str(c);
        }
        s.push(')');
        Cow::Owned(s)
    }
}

impl<A> KeyBind for A
where
    A: KeyScalar + for<'a> Encode<'a, SqliteDb> + Type<SqliteDb> + Send,
{
    const ARITY: usize = 1;

    fn bind_key<'q, Q: BindQuery<'q>>(self, query: Q) -> Q
    where
        Self: 'q,
    {
        query.bind_value(self)
    }

    fn push_row(self, qb: &mut QueryBuilder<SqliteDb>) {
        qb.push_bind(self);
    }
}

macro_rules! impl_key_bind_tuple {
    ($( ($t0:ident $i0:tt $(, $t:ident $i:tt)*) ),+ $(,)?) => {
        $(
            impl<$t0 $(, $t)*> KeyBind for ($t0, $($t,)*)
            where
                $t0: for<'a> Encode<'a, SqliteDb> + Type<SqliteDb> + Send,
                $($t: for<'a> Encode<'a, SqliteDb> + Type<SqliteDb> + Send,)*
            {
                const ARITY: usize = [$i0 $(, $i)*].len();

                fn bind_key<'q, Q: BindQuery<'q>>(self, query: Q) -> Q
                where
                    Self: 'q,
                {
                    query.bind_value(self.$i0) $(.bind_value(self.$i))*
                }

                fn push_row(self, qb: &mut QueryBuilder<SqliteDb>) {
                    qb.push("(").push_bind(self.$i0);
                    $(
                        qb.push(", ").push_bind(self.$i);
                    )*
                    qb.push(")");
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

/// The separator between items in a comma-separated list (column lists, SET clauses). [`table!`]
/// emits it after every item, then drops the trailing one with
/// [`strip_tail`](crate::string::strip_tail); the two share this single definition so the string
/// and the byte count can never disagree.
///
/// Do **not** use this directly.
pub const SEP_COMMA: &str = ", ";

/// The separator between AND-ed predicates in an `UPDATE ... WHERE` clause. Emitted-then-stripped
/// like [`SEP_COMMA`].
///
/// Do **not** use this directly.
pub const SEP_AND: &str = " AND ";

/// The `INSERT [OR IGNORE|REPLACE] INTO ` verb for a conflict mode. Used by the
/// [`table!`]-generated `SQL_INSERT_PREFIX`.
///
/// Do **not** use this directly.
pub const fn insert_verb(conflict: Conflict) -> &'static str {
    match conflict {
        Conflict::Ignore => "INSERT OR IGNORE INTO ",
        Conflict::Replace => "INSERT OR REPLACE INTO ",
        Conflict::Upsert => "INSERT INTO ",
    }
}

/// `s` if `cond`, else `""`. Lets [`table!`] conditionally include a column's SQL fragment at
/// const-eval (macros can't filter columns by key-membership).
///
/// Do **not** use this directly.
pub const fn keep(cond: bool, s: &'static str) -> &'static str {
    if cond {
        s
    } else {
        ""
    }
}

/// Whether `schema`'s key columns, read in column order, are exactly `declared` in the same order.
/// Lets [`table!`] reject a `key` list written out of column order at compile time, so the declared
/// order can never disagree with the (column) order everything actually binds and generates SQL in.
///
/// Do **not** use this directly.
#[must_use]
pub const fn keys_in_column_order(schema: &Schema, declared: &[&str]) -> bool {
    let cols = schema.columns;
    let mut ci = 0;
    let mut di = 0;
    while ci < cols.len() {
        if cols[ci].key {
            if di >= declared.len() || !crate::string::str_eq(cols[ci].name, declared[di]) {
                return false;
            }
            di += 1;
        }
        ci += 1;
    }
    di == declared.len()
}

/// Whether `s` is a bare SQL identifier (`[A-Za-z_][A-Za-z0-9_]*`). Lets [`table!`] reject a table
/// name that would need quoting, since names are spliced into SQL unquoted.
///
/// Do **not** use this directly.
#[doc(hidden)]
pub const fn is_sql_ident(s: &str) -> bool {
    let b = s.as_bytes();
    if b.is_empty() {
        return false;
    }
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        let ok = c == b'_' || c.is_ascii_alphabetic() || (i > 0 && c.is_ascii_digit());
        if !ok {
            return false;
        }
        i += 1;
    }
    true
}

/// Whether `schema` has a column named `name`. Lets [`ColRef::new`] reject a reference to a column
/// the table does not have, at compile time.
const fn has_column(schema: &Schema, name: &str) -> bool {
    let cols = schema.columns;
    let mut i = 0;
    while i < cols.len() {
        if crate::string::str_eq(cols[i].name, name) {
            return true;
        }
        i += 1;
    }
    false
}

/// A view into a table scoped to a caller-owned transaction, created by [`TableView::on`]. Every
/// operation runs on the borrowed transaction; commit or roll back the transaction yourself.
pub struct TxView<'a, 'c, T: Table> {
    view: &'a TableView<T>,
    tx: &'a mut Transaction<'c, SqliteDb>,
}

impl<T: Table> TxView<'_, '_, T> {
    /// Like [`TableView::insert_bulk`], but on the borrowed transaction.
    #[instrument(level = "trace", skip_all)]
    pub async fn insert_bulk<'i>(
        &mut self,
        items: impl IntoIterator<Item = &'i T>,
    ) -> sqlx::Result<()>
    where
        T: 'i,
    {
        let mut it = items.into_iter().peekable();
        if it.peek().is_none() {
            return Ok(());
        }

        let bind_cols = T::SCHEMA.bind_col_count();
        let per = (self.view.sqlite.info().await.variable_number_limit / bind_cols.max(1)).max(1);

        while it.peek().is_some() {
            let mut qb = sqlx::QueryBuilder::<SqliteDb>::new(T::SQL_INSERT_PREFIX);
            qb.push_values(it.by_ref().take(per), |mut sep, item: &T| {
                item.bind_row(&mut sep);
            });
            // `""` for ignore/replace (their conflict handling is in the verb).
            qb.push(T::SQL_INSERT_SUFFIX);
            qb.build().execute(&mut **self.tx).await?;
        }
        Ok(())
    }

    /// Like [`TableView::delete_all`], but on the borrowed transaction; returns
    /// the number of rows deleted.
    #[instrument(level = "trace", skip_all)]
    pub async fn delete_all(&mut self) -> sqlx::Result<u64> {
        self.exec().delete_all().await
    }

    /// Like [`TableView::delete`], but on the borrowed transaction.
    #[instrument(level = "trace", skip_all)]
    pub async fn delete<K: KeyBind>(&mut self, key: K) -> sqlx::Result<()> {
        self.exec().delete(key).await
    }

    /// Like [`TableView::delete_by`], but on the borrowed transaction.
    #[instrument(level = "trace", skip_all)]
    pub async fn delete_by<V>(&mut self, col: ColRef<T>, value: V) -> sqlx::Result<()>
    where
        V: Send + for<'e> Encode<'e, SqliteDb> + Type<SqliteDb>,
    {
        self.exec().delete_eq(col.name(), value).await
    }

    /// Like [`TableView::update_one`], but on the borrowed transaction.
    #[instrument(level = "trace", skip_all)]
    pub async fn update_one(&mut self, row: &T) -> sqlx::Result<()> {
        self.exec().update_one(row).await
    }

    /// A single-statement [`ExecView`] over this transaction's connection.
    fn exec(&mut self) -> ExecView<&mut sqlx::sqlite::SqliteConnection, T> {
        ExecView {
            exec: &mut **self.tx,
            _t: PhantomData,
        }
    }
}

/// A generic view over a "transaction provider".
///
/// A "transaction provider" is either [`sqlx::Transaction`] or a [`sqlx::Pool`].
struct ExecView<E, T> {
    exec: E,
    _t: PhantomData<T>,
}

impl<E, T: Table> ExecView<E, T> {
    async fn delete<'e, K: KeyBind>(self, key: K) -> sqlx::Result<()>
    where
        E: sqlx::Executor<'e, Database = SqliteDb>,
    {
        const {
            assert!(
                K::ARITY == T::KEY_COLS.len(),
                "delete requires the full key; the key arity must match the table's key columns",
            );
        }
        key.bind_key(sqlx::query(T::SQL_DELETE_BY_KEY)).execute(self.exec).await?;
        Ok(())
    }

    async fn delete_all<'e>(self) -> sqlx::Result<u64>
    where
        E: sqlx::Executor<'e, Database = SqliteDb>,
    {
        let result = sqlx::query(T::SQL_DELETE_ALL).execute(self.exec).await?;
        Ok(result.rows_affected())
    }

    async fn update_one<'e>(self, row: &T) -> sqlx::Result<()>
    where
        E: sqlx::Executor<'e, Database = SqliteDb>,
    {
        const {
            assert!(
                T::SCHEMA.bind_col_count() > T::KEY_COLS.len(),
                "update_one requires at least one non-key column to set",
            );
        }
        let mut qb = sqlx::QueryBuilder::<SqliteDb>::new(T::SQL_UPDATE);
        row.bind_update(qb.build()).execute(self.exec).await?;
        Ok(())
    }

    async fn count<'e>(self) -> sqlx::Result<u64>
    where
        E: sqlx::Executor<'e, Database = SqliteDb>,
    {
        let (n,): (i64,) = sqlx::query_as::<_, (i64,)>(T::SQL_COUNT).fetch_one(self.exec).await?;
        Ok(n as u64)
    }

    async fn delete_eq<'e, V>(self, col: &str, value: V) -> sqlx::Result<()>
    where
        E: sqlx::Executor<'e, Database = SqliteDb>,
        V: Send + for<'a> Encode<'a, SqliteDb> + Type<SqliteDb>,
    {
        let mut qb = sqlx::QueryBuilder::<SqliteDb>::new(format!(
            "DELETE FROM {} WHERE {col} = ",
            T::SCHEMA.name,
        ));
        qb.push_bind(value);
        qb.build().execute(self.exec).await?;
        Ok(())
    }

    async fn count_eq<'e, V>(self, col: &str, value: V) -> sqlx::Result<u64>
    where
        E: sqlx::Executor<'e, Database = SqliteDb>,
        V: Send + for<'a> Encode<'a, SqliteDb> + Type<SqliteDb>,
    {
        let mut qb = sqlx::QueryBuilder::<SqliteDb>::new(format!(
            "SELECT COUNT(*) FROM {} WHERE {col} = ",
            T::SCHEMA.name,
        ));
        qb.push_bind(value);
        let (n,): (i64,) = qb.build_query_as::<(i64,)>().fetch_one(self.exec).await?;
        Ok(n as u64)
    }
}

impl<T: Table> TableView<T> {
    /// Scope this table's write operations to a caller-owned `tx`, returning a [`TxView`].
    pub fn on<'b, 'c>(&'b self, tx: &'b mut Transaction<'c, SqliteDb>) -> TxView<'b, 'c, T> {
        TxView { view: self, tx }
    }

    /// Insert multiple `item`s into the table.
    ///
    /// This function will start a new transaction for you.
    ///
    /// ```text
    /// insert_bulk([r1, r2])  =>
    ///   INSERT INTO kv3 (ns, k, v) VALUES (?, ?, ?), (?, ?, ?)
    ///     ON CONFLICT (ns, k) DO UPDATE SET v = EXCLUDED.v
    /// // one statement per chunk (bind-var limit);
    /// // conflict ignore/replace => `insert or ignore/replace ...`, no `on conflict` suffix
    /// ```
    #[instrument(level = "trace", skip_all)]
    pub async fn insert_bulk<'a>(&self, items: impl IntoIterator<Item = &'a T>) -> sqlx::Result<()>
    where
        T: 'a,
    {
        let mut tx = self.sqlite.pool().begin().await?;
        self.on(&mut tx).insert_bulk(items).await?;
        tx.commit().await
    }

    /// Insert one `item` into the table.
    ///
    /// **If you are trying to insert multiple rows, do not use this function.** See
    /// [`Self::insert_bulk`].
    ///
    /// ```text
    /// insert_one(&r)  =>
    ///   INSERT INTO kv3 (ns, k, v) VALUES (?, ?, ?)
    ///     ON CONFLICT (ns, k) DO UPDATE SET v = EXCLUDED.v
    /// ```
    #[instrument(level = "trace", skip_all)]
    pub async fn insert_one(&self, item: &T) -> sqlx::Result<()> {
        self.insert_bulk(std::iter::once(item)).await
    }

    /// Delete the row with this key.
    ///
    /// `key` must be the full key; its arity must match the table's key columns (checked at compile
    /// time), so it matches at most one row. To delete every row sharing a column value, use
    /// [`Self::delete_by`] or [`Self::delete_in`].
    ///
    /// ```text
    /// delete(("app", 3))  =>  DELETE FROM kv3 WHERE ns = ? AND k = ?
    /// ```
    #[instrument(level = "trace", skip_all)]
    pub async fn delete<K: KeyBind>(&self, key: K) -> sqlx::Result<()> {
        self.exec().delete(key).await
    }

    /// Drop all rows from this database.
    ///
    /// ```text
    /// delete_all()  =>  DELETE FROM kv3
    /// ```
    #[instrument(level = "trace", skip_all)]
    pub async fn delete_all(&self) -> sqlx::Result<()> {
        self.exec().delete_all().await?;
        Ok(())
    }

    /// Delete every row matching one of `keys`, batched into one statement per chunk (sized to the
    /// bind-variable limit) rather than a statement per key.
    ///
    /// ```text
    /// delete_many([k1, k2])  =>  DELETE FROM kv3 WHERE (ns, k) IN ((?, ?), (?, ?))
    /// // chunked to the bind-var limit; scalar key => `... where id in (?, ?)`
    /// ```
    #[instrument(level = "trace", skip_all)]
    pub async fn delete_many<K: KeyBind>(
        &self,
        keys: impl IntoIterator<Item = K>,
    ) -> sqlx::Result<()> {
        const {
            assert!(
                K::ARITY == T::KEY_COLS.len(),
                "delete_many takes full keys; the key arity must match the table's key columns",
            );
        }
        self.wipe_in(K::in_lhs(T::KEY_COLS), keys).await
    }

    /// Delete every row whose `col` is one of `values` - the column counterpart to
    /// [`Self::delete_many`], chunked to the bind-variable limit. An empty iterator is a no-op.
    ///
    /// ```text
    /// delete_in(Kv3::v, [a, b])  =>  DELETE FROM kv3 WHERE v IN (?, ?)
    /// ```
    #[instrument(level = "trace", skip_all)]
    pub async fn delete_in<V: KeyBind>(
        &self,
        col: ColRef<T>,
        values: impl IntoIterator<Item = V>,
    ) -> sqlx::Result<()> {
        const {
            assert!(V::ARITY == 1, "delete_in matches a single column; values must be scalar");
        }
        self.wipe_in(Cow::Borrowed(col.name()), values).await
    }

    /// `DELETE FROM <table> WHERE <lhs> IN (...)`, chunked to the bind-variable
    /// limit. Shared by [`Self::delete_many`] and [`Self::delete_in`].
    async fn wipe_in<K: KeyBind>(
        &self,
        lhs: Cow<'static, str>,
        values: impl IntoIterator<Item = K>,
    ) -> sqlx::Result<()> {
        let per_chunk = (self.sqlite.info().await.variable_number_limit / K::ARITY.max(1)).max(1);
        let sql_prefix = format!("DELETE FROM {} WHERE {lhs} IN (", T::SCHEMA.name);
        let mut values = values.into_iter().peekable();
        while values.peek().is_some() {
            let mut qb = sqlx::QueryBuilder::<SqliteDb>::new(sql_prefix.as_str());
            let mut first = true;
            for v in values.by_ref().take(per_chunk) {
                if !first {
                    qb.push(", ");
                }
                first = false;
                v.push_row(&mut qb);
            }
            qb.push(")");
            qb.build().execute(self.sqlite.pool()).await?;
        }
        Ok(())
    }

    /// Number of rows in the table (`SELECT COUNT(*)`).
    ///
    /// ```text
    /// count()  =>  SELECT COUNT(*) FROM kv3
    /// ```
    #[instrument(level = "trace", skip_all)]
    pub async fn count(&self) -> sqlx::Result<u64> {
        self.exec().count().await
    }

    /// Delete every row where `col` equals `value` (`DELETE ... WHERE col = ?`).
    ///
    /// ```text
    /// delete_by(Kv3::v, x)  =>  DELETE FROM kv3 WHERE v = ?
    /// ```
    #[instrument(level = "trace", skip_all)]
    pub async fn delete_by<V>(&self, col: ColRef<T>, value: V) -> sqlx::Result<()>
    where
        V: Send + for<'e> Encode<'e, SqliteDb> + Type<SqliteDb>,
    {
        self.exec().delete_eq(col.name(), value).await
    }

    /// Number of rows where `col` equals `value` (`SELECT COUNT(*) ... WHERE col = ?`).
    ///
    /// ```text
    /// count_by(Kv3::v, x)  =>  SELECT COUNT(*) FROM kv3 WHERE v = ?
    /// ```
    #[instrument(level = "trace", skip_all)]
    pub async fn count_by<V>(&self, col: ColRef<T>, value: V) -> sqlx::Result<u64>
    where
        V: Send + for<'e> Encode<'e, SqliteDb> + Type<SqliteDb>,
    {
        self.exec().count_eq(col.name(), value).await
    }

    /// Update a full row in place, matched by its key: sets every non-key
    /// column to `row`'s value and matches on the key columns. Unlike a
    /// `conflict: replace` insert, this is a real `update` - it never deletes the
    /// row, so it won't fire delete triggers or cascade to child tables.
    ///
    /// The table must have at least one non-key column (otherwise there is
    /// nothing to set).
    ///
    /// ```text
    /// update_one(&r)  =>  UPDATE kv3 SET v = ? WHERE ns = ? AND k = ?
    /// ```
    #[instrument(level = "trace", skip_all)]
    pub async fn update_one(&self, row: &T) -> sqlx::Result<()> {
        self.exec().update_one(row).await
    }

    /// A single-statement [`ExecView`] over this table's connection pool.
    fn exec(&self) -> ExecView<&sqlx::sqlite::SqlitePool, T> {
        ExecView {
            exec: self.sqlite.pool(),
            _t: PhantomData,
        }
    }
}

impl<T> TableView<T>
where
    T: Table + for<'r> sqlx::FromRow<'r, sqlx::sqlite::SqliteRow> + Send + Unpin,
{
    /// Get the row with this key.
    ///
    /// `key` must be the full key; its arity must match the table's key columns (checked at compile
    /// time), so it matches at most one row. To fetch by a non-key column, use [`Self::get_by`].
    ///
    /// ```text
    /// get(("app", 3))  =>  SELECT ns, k, v FROM kv3 WHERE ns = ? AND k = ?
    /// ```
    #[instrument(level = "trace", skip_all)]
    pub async fn get<K: KeyBind>(&self, key: K) -> sqlx::Result<Option<T>> {
        const {
            assert!(
                K::ARITY == T::KEY_COLS.len(),
                "get requires the full key; the key arity must match the table's key columns",
            );
        }
        key.bind_key(sqlx::query_as::<_, T>(T::SQL_GET)).fetch_optional(self.sqlite.pool()).await
    }

    /// Fetch the first row where `col` equals `value` (`... WHERE col = ? LIMIT 1`).
    ///
    /// ```text
    /// get_by(Kv3::v, x)  =>  SELECT ns, k, v FROM kv3 WHERE v = ? LIMIT 1
    /// ```
    #[instrument(level = "trace", skip_all)]
    pub async fn get_by<V>(&self, col: ColRef<T>, value: V) -> sqlx::Result<Option<T>>
    where
        V: Send + for<'e> Encode<'e, SqliteDb> + Type<SqliteDb>,
    {
        let sql = format!(
            "SELECT {} FROM {} WHERE {} = ? LIMIT 1",
            T::SQL_COLS,
            T::SCHEMA.name,
            col.name(),
        );
        sqlx::query_as::<_, T>(sqlx::AssertSqlSafe(sql))
            .bind(value)
            .fetch_optional(self.sqlite.pool())
            .await
    }

    /// Stream every row where `col` equals `value`, ordered by key.
    ///
    /// ```text
    /// filter_by(Kv3::v, x)  =>  SELECT ns, k, v FROM kv3 WHERE v = ? ORDER BY ns, k
    /// ```
    pub fn filter_by<'a, V>(
        &'a self,
        col: ColRef<T>,
        value: V,
    ) -> impl futures::Stream<Item = sqlx::Result<T>> + Send + 'a
    where
        V: Send + for<'e> Encode<'e, SqliteDb> + Type<SqliteDb> + 'a,
    {
        let sql = format!(
            "SELECT {} FROM {} WHERE {} = ? ORDER BY {}",
            T::SQL_COLS,
            T::SCHEMA.name,
            col.name(),
            T::SQL_KEYS_CSV,
        );
        sqlx::query_as::<_, T>(sqlx::AssertSqlSafe(sql)).bind(value).fetch(self.sqlite.pool())
    }

    /// Stream every row whose key is one of `keys`, chunked to the bind-var limit.
    ///
    /// ```text
    /// get_many([k1, k2])  =>  SELECT ns, k, v FROM kv3 WHERE (ns, k) IN ((?, ?), (?, ?))
    /// ```
    pub fn get_many<'a, K, I>(
        &'a self,
        keys: I,
    ) -> impl futures::Stream<Item = sqlx::Result<T>> + Send + 'a
    where
        K: KeyBind + Send + 'a,
        I: IntoIterator<Item = K> + Send + 'a,
        I::IntoIter: Send,
    {
        const {
            assert!(
                K::ARITY == T::KEY_COLS.len(),
                "get_many takes full keys; the key arity must match the table's key columns",
            );
        }
        self.scan_in(K::in_lhs(T::KEY_COLS), keys)
    }

    /// Stream every row whose `col` is one of `values` - the column counterpart to
    /// [`Self::get_many`], chunked to the bind-variable limit.
    ///
    /// ```text
    /// filter_in(Kv3::v, [a, b])  =>  SELECT ns, k, v FROM kv3 WHERE v IN (?, ?)
    /// ```
    pub fn filter_in<'a, V, I>(
        &'a self,
        col: ColRef<T>,
        values: I,
    ) -> impl futures::Stream<Item = sqlx::Result<T>> + Send + 'a
    where
        V: KeyBind + Send + 'a,
        I: IntoIterator<Item = V> + Send + 'a,
        I::IntoIter: Send,
    {
        const {
            assert!(V::ARITY == 1, "filter_in matches a single column; values must be scalar");
        }
        self.scan_in(Cow::Borrowed(col.name()), values)
    }

    /// `SELECT * FROM <table> WHERE <lhs> IN (...)`, streamed and chunked to the bind-variable
    /// limit. Shared by [`Self::get_many`] and [`Self::filter_in`].
    fn scan_in<'a, K, I>(
        &'a self,
        lhs: Cow<'static, str>,
        values: I,
    ) -> impl futures::Stream<Item = sqlx::Result<T>> + Send + 'a
    where
        K: KeyBind + Send + 'a,
        I: IntoIterator<Item = K> + Send + 'a,
        I::IntoIter: Send,
    {
        use futures::TryStreamExt;

        let sqlite = &self.sqlite;
        let name = T::SCHEMA.name;
        async_stream::try_stream! {
            let per_chunk = (sqlite.info().await.variable_number_limit / K::ARITY.max(1)).max(1);

            let mut values = values.into_iter().peekable();
            while values.peek().is_some() {
                let mut qb = sqlx::QueryBuilder::<SqliteDb>::new(format!(
                    "SELECT {} FROM {name} WHERE {lhs} IN (",
                    T::SQL_COLS,
                ));
                let mut first = true;
                for v in values.by_ref().take(per_chunk) {
                    if !first {
                        qb.push(", ");
                    }
                    first = false;
                    v.push_row(&mut qb);
                }
                qb.push(")");
                let mut rows = qb.build_query_as::<T>().fetch(sqlite.pool());
                while let Some(row) = rows.try_next().await? {
                    yield row;
                }
            }
        }
    }

    /// Stream all rows.
    ///
    /// If you want a [`Vec`] out of this, call `.try_collect()`.
    ///
    /// ```text
    /// all()  =>  SELECT ns, k, v FROM kv3
    /// ```
    pub fn all(&self) -> impl futures::Stream<Item = sqlx::Result<T>> + Send + '_ {
        sqlx::query_as::<_, T>(T::SQL_SELECT_ALL).fetch(self.sqlite.pool())
    }

    /// Stream all rows, ordered by key.
    ///
    /// If you want a [`Vec`], call `.try_collect()`.
    ///
    /// ```text
    /// all_ordered()  =>  SELECT ns, k, v FROM kv3 ORDER BY ns, k
    /// ```
    pub fn all_ordered(&self) -> impl futures::Stream<Item = sqlx::Result<T>> + Send + '_ {
        sqlx::query_as::<_, T>(T::SQL_SELECT_ALL_ORDERED).fetch(self.sqlite.pool())
    }
}

#[cfg(test)]
mod tests {
    use futures::TryStreamExt;
    use rstest::{fixture, rstest};

    use super::*;

    mod macro_hygiene {
        use rstest::rstest;

        use crate::sqlite::Table;

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
            key: ["host", "id"],
            conflict: ignore,
            columns: {
                host    => |e| e.host.as_str(),
                id      => |e| e.id.as_str(),
                body    => |e| e.body.as_str(),
                created => raw(|_| "strftime('%s','now')"),
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

        struct Ups {
            a: String,
            b: String,
        }
        crate::table!(Ups {
            name: "ups",
            key: "a",
            conflict: upsert,
            columns: {
                a  => |e| e.a.as_str(),
                b  => |e| e.b.as_str(),
                at => raw(|_| "strftime('%s','now')"),
            },
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
            assert!(matches!(Event::SCHEMA.columns[3].kind, ColKind::Expr));

            // The macro emits a `ColRef` const per column (Expr columns included).
            assert_eq!(Kv3::ns.name(), "ns");
            assert_eq!(Kv3::v.name(), "v");
            assert_eq!(Event::created.name(), "created");
            assert_eq!(Single::val.name(), "val");
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

        // Every statement the macro precomputes, across single/composite keys,
        // all three conflict verbs, and an Expr column (in the upsert SET, out
        // of the UPDATE SET). `ignore`/`replace` carry no insert suffix - their
        // conflict handling is in the verb.
        #[rstest]
        #[case(Kv3::SQL_COLS, "ns, k, v")]
        #[case(Kv3::SQL_KEYS_CSV, "ns, k")]
        #[case(Kv3::SQL_SELECT_ALL, "SELECT ns, k, v FROM kv3")]
        #[case(Kv3::SQL_SELECT_ALL_ORDERED, "SELECT ns, k, v FROM kv3 ORDER BY ns, k")]
        #[case(Kv3::SQL_COUNT, "SELECT COUNT(*) FROM kv3")]
        #[case(Kv3::SQL_DELETE_ALL, "DELETE FROM kv3")]
        #[case(Kv3::SQL_INSERT_PREFIX, "INSERT INTO kv3(ns, k, v) ")]
        #[case(Kv3::SQL_INSERT_SUFFIX, " ON CONFLICT (ns, k) DO UPDATE SET v = EXCLUDED.v")]
        #[case(Kv3::SQL_UPDATE, "UPDATE kv3 SET v = ? WHERE ns = ? AND k = ?")]
        #[case(Kv3::SQL_WHERE_KEY, "ns = ? AND k = ?")]
        #[case(Kv3::SQL_GET, "SELECT ns, k, v FROM kv3 WHERE ns = ? AND k = ?")]
        #[case(Kv3::SQL_DELETE_BY_KEY, "DELETE FROM kv3 WHERE ns = ? AND k = ?")]
        #[case(Single::SQL_GET, "SELECT id, val FROM single WHERE id = ?")]
        #[case(Single::SQL_DELETE_BY_KEY, "DELETE FROM single WHERE id = ?")]
        #[case(Event::SQL_INSERT_PREFIX, "INSERT OR IGNORE INTO events(host, id, body, created) ")]
        #[case(Event::SQL_INSERT_SUFFIX, "")]
        #[case(Event::SQL_UPDATE, "UPDATE events SET body = ? WHERE host = ? AND id = ?")]
        #[case(Single::SQL_SELECT_ALL_ORDERED, "SELECT id, val FROM single ORDER BY id")]
        #[case(Single::SQL_INSERT_PREFIX, "INSERT OR REPLACE INTO single(id, val) ")]
        #[case(Single::SQL_INSERT_SUFFIX, "")]
        #[case(Single::SQL_UPDATE, "UPDATE single SET val = ? WHERE id = ?")]
        #[case(Ups::SQL_SELECT_ALL, "SELECT a, b, at FROM ups")]
        #[case(
            Ups::SQL_INSERT_SUFFIX,
            " ON CONFLICT (a) DO UPDATE SET b = EXCLUDED.b, at = EXCLUDED.at"
        )]
        #[case(super::super::Kv::SQL_SELECT_ALL_ORDERED, "SELECT id, val FROM kv ORDER BY id")]
        fn generated_sql_is_correct(#[case] actual: &str, #[case] expected: &str) {
            assert_eq!(actual, expected);
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
    fn bind_col_count_excludes_expr_columns() {
        struct Toy;
        impl Table for Toy {
            const SCHEMA: Schema = Schema {
                name: "toy",
                columns: &[Col::key("id"), Col::col("body"), Col::expr("at")],
                conflict: Conflict::Upsert,
            };
            const KEY_COLS: &'static [&'static str] = &["id"];
            const SQL_COLS: &'static str = "id, body, at";
            const SQL_KEYS_CSV: &'static str = "id";
            const SQL_SELECT_ALL: &'static str = "SELECT id, body, at FROM toy";
            const SQL_SELECT_ALL_ORDERED: &'static str = "SELECT id, body, at FROM toy ORDER BY id";
            const SQL_COUNT: &'static str = "SELECT COUNT(*) FROM toy";
            const SQL_DELETE_ALL: &'static str = "DELETE FROM toy";
            const SQL_INSERT_PREFIX: &'static str = "INSERT INTO toy(id, body, at) ";
            const SQL_INSERT_SUFFIX: &'static str =
                " ON CONFLICT (id) DO UPDATE SET body = EXCLUDED.body, at = EXCLUDED.at";
            const SQL_UPDATE: &'static str = "UPDATE toy SET body = ? WHERE id = ?";
            const SQL_WHERE_KEY: &'static str = "id = ?";
            const SQL_GET: &'static str = "SELECT id, body, at FROM toy WHERE id = ?";
            const SQL_DELETE_BY_KEY: &'static str = "DELETE FROM toy WHERE id = ?";
            fn bind_row(
                &self,
                sep: &mut sqlx::query_builder::Separated<'_, SqliteDb, &'static str>,
            ) {
                sep.push_bind("x").push_bind("y").push("strftime('%s','now')");
            }
            fn bind_update<'q>(
                &'q self,
                query: sqlx::query::Query<'q, sqlx::Sqlite, sqlx::sqlite::SqliteArguments>,
            ) -> sqlx::query::Query<'q, sqlx::Sqlite, sqlx::sqlite::SqliteArguments> {
                query.bind("y").bind("x") // SET body, WHERE id
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
        const KEY_COLS: &'static [&'static str] = &["id"];
        const SQL_COLS: &'static str = "id, val";
        const SQL_KEYS_CSV: &'static str = "id";
        const SQL_SELECT_ALL: &'static str = "SELECT id, val FROM kv";
        const SQL_SELECT_ALL_ORDERED: &'static str = "SELECT id, val FROM kv ORDER BY id";
        const SQL_COUNT: &'static str = "SELECT COUNT(*) FROM kv";
        const SQL_DELETE_ALL: &'static str = "DELETE FROM kv";
        const SQL_INSERT_PREFIX: &'static str = "INSERT INTO kv(id, val) ";
        const SQL_INSERT_SUFFIX: &'static str =
            " ON CONFLICT (id) DO UPDATE SET val = EXCLUDED.val";
        const SQL_UPDATE: &'static str = "UPDATE kv SET val = ? WHERE id = ?";
        const SQL_WHERE_KEY: &'static str = "id = ?";
        const SQL_GET: &'static str = "SELECT id, val FROM kv WHERE id = ?";
        const SQL_DELETE_BY_KEY: &'static str = "DELETE FROM kv WHERE id = ?";
        fn bind_row(&self, sep: &mut sqlx::query_builder::Separated<'_, SqliteDb, &'static str>) {
            sep.push_bind(self.id.as_str()).push_bind(self.val.as_str());
        }
        fn bind_update<'q>(
            &'q self,
            query: sqlx::query::Query<'q, sqlx::Sqlite, sqlx::sqlite::SqliteArguments>,
        ) -> sqlx::query::Query<'q, sqlx::Sqlite, sqlx::sqlite::SqliteArguments> {
            query.bind(self.val.as_str()).bind(self.id.as_str()) // SET val, WHERE id
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
        sqlx::query("CREATE TABLE kv(id TEXT PRIMARY KEY, val TEXT)")
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
    async fn column_accessors(#[future(awt)] store: TableView<Kv>) {
        store
            .insert_bulk([&Kv::new("a", "x"), &Kv::new("b", "x"), &Kv::new("c", "y")])
            .await
            .unwrap();

        // A column that isn't the key - reachable only via the `*_by` accessors.
        let val = ColRef::<Kv>::new("val");

        // count_by / get_by
        assert_eq!(store.count_by(val, "x").await.unwrap(), 2);
        assert_eq!(store.count_by(val, "z").await.unwrap(), 0);
        assert!(store.get_by(val, "y").await.unwrap().is_some());
        assert!(store.get_by(val, "z").await.unwrap().is_none());

        // filter_by streams every match, ordered by key.
        let xs: Vec<String> =
            store.filter_by(val, "x").map_ok(|r| r.id).try_collect().await.unwrap();
        assert_eq!(xs, ["a".to_string(), "b".to_string()]);

        // filter_in over several values (unordered, so sort before comparing).
        let mut got: Vec<String> =
            store.filter_in(val, ["x", "y"]).map_ok(|r| r.id).try_collect().await.unwrap();
        got.sort();
        assert_eq!(got, ["a".to_string(), "b".to_string(), "c".to_string()]);

        // delete_by removes every match; delete_in clears the rest.
        store.delete_by(val, "x").await.unwrap();
        assert_eq!(ids(&store).await, ["c".to_string()]);
        store.delete_in(val, ["y"]).await.unwrap();
        assert!(ids(&store).await.is_empty());
    }

    // `update_one` needs the macro-generated `bind_update`, so this uses a
    // `table!` type - an `ignore`-conflict one, to show update works where an
    // insert would refuse to overwrite.
    #[tokio::test]
    async fn update_one_updates_in_place() {
        struct Rec {
            id: String,
            val: String,
        }
        crate::table!(Rec {
            name: "upd",
            key: "id",
            conflict: ignore,
            columns: {
                id  => |r| r.id.as_str(),
                val => |r| r.val.as_str(),
            }
        });
        impl<'r> sqlx::FromRow<'r, sqlx::sqlite::SqliteRow> for Rec {
            fn from_row(row: &'r sqlx::sqlite::SqliteRow) -> sqlx::Result<Self> {
                use sqlx::Row;
                Ok(Self {
                    id: row.try_get("id")?,
                    val: row.try_get("val")?,
                })
            }
        }

        let sqlite = crate::sqlite::Sqlite::builder().memory().open().await.unwrap();
        sqlx::query("CREATE TABLE upd(id TEXT PRIMARY KEY, val TEXT)")
            .execute(sqlite.pool())
            .await
            .unwrap();
        let view = TableView::<Rec>::new(sqlite);

        view.insert_one(&Rec {
            id: "a".into(),
            val: "1".into(),
        })
        .await
        .unwrap();
        view.update_one(&Rec {
            id: "a".into(),
            val: "2".into(),
        })
        .await
        .unwrap();
        assert_eq!(view.get("a").await.unwrap().unwrap().val, "2");

        // Updating a missing key affects nothing - no error, and no insert.
        view.update_one(&Rec {
            id: "zzz".into(),
            val: "x".into(),
        })
        .await
        .unwrap();
        assert!(view.get("zzz").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn raw_column_computes_sql_from_the_object() {
        struct Ev {
            id: String,
            n: i64,
        }
        crate::table!(Ev {
            name: "rawtest",
            key: "id",
            conflict: ignore,
            columns: {
                id      => |e| e.id.as_str(),
                doubled => raw(|e| format!("{} * 2", e.n)),
            }
        });

        let sqlite = crate::sqlite::Sqlite::builder().memory().open().await.unwrap();
        sqlx::query("CREATE TABLE rawtest(id TEXT PRIMARY KEY, doubled INTEGER)")
            .execute(sqlite.pool())
            .await
            .unwrap();
        let view = TableView::<Ev>::new(sqlite);

        view.insert_one(&Ev {
            id: "a".into(),
            n: 21,
        })
        .await
        .unwrap();

        // The lambda saw `n = 21` and emitted the raw SQL `21 * 2`, which sqlite
        // evaluated on insert.
        let doubled: i64 = sqlx::query_scalar("SELECT doubled FROM rawtest WHERE id = 'a'")
            .fetch_one(view.sqlite().pool())
            .await
            .unwrap();
        assert_eq!(doubled, 42);
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
    async fn count_and_delete_many(#[future(awt)] store: TableView<Kv>) {
        assert_eq!(store.count().await.unwrap(), 0);
        store.delete_many(std::iter::empty::<&str>()).await.unwrap(); // empty is a no-op

        let rows: Vec<Kv> = ["a", "b", "c", "d"].into_iter().map(|id| Kv::new(id, "v")).collect();
        store.insert_bulk(rows.iter()).await.unwrap();
        assert_eq!(store.count().await.unwrap(), 4);

        store.delete_many(["a", "c"]).await.unwrap();
        assert_eq!(store.count().await.unwrap(), 2);
        assert_eq!(ids(&store).await, ["b", "d"].map(String::from));

        // values that aren't present are simply skipped
        store.delete_many(["zzz"]).await.unwrap();
        assert_eq!(store.count().await.unwrap(), 2);
    }

    #[rstest]
    #[tokio::test]
    async fn get_many_fetches_requested_keys(#[future(awt)] store: TableView<Kv>) {
        let empty: Vec<Kv> =
            store.get_many(std::iter::empty::<&str>()).try_collect().await.unwrap();
        assert!(empty.is_empty());

        let rows: Vec<Kv> = ["a", "b", "c"].into_iter().map(|id| Kv::new(id, "v")).collect();
        store.insert_bulk(rows.iter()).await.unwrap();

        let mut got: Vec<Kv> = store.get_many(["a", "c", "missing"]).try_collect().await.unwrap();
        got.sort();
        assert_eq!(got, vec![Kv::new("a", "v"), Kv::new("c", "v")]);
    }

    #[rstest]
    #[tokio::test]
    async fn delete_all_tx_reports_count(#[future(awt)] store: TableView<Kv>) {
        store.insert_bulk([&Kv::new("a", "1"), &Kv::new("b", "2")]).await.unwrap();
        let mut tx = store.sqlite().pool().begin().await.unwrap();
        let deleted = store.on(&mut tx).delete_all().await.unwrap();
        tx.commit().await.unwrap();
        assert_eq!(deleted, 2);
        assert!(ids(&store).await.is_empty());
    }

    #[rstest]
    #[tokio::test]
    async fn insert_bulk_spans_multiple_chunks(#[future(awt)] store: TableView<Kv>) {
        // Size the row count off the real, driver-reported bind-variable limit
        // (never weaken it) so `insert_bulk`'s `while` loop is forced to run
        // several times regardless of the sqlite build.
        let limit = store.sqlite().info().await.variable_number_limit;
        let n = limit * 2 + 137;
        let rows: Vec<Kv> = (0..n).map(|i| Kv::new(&format!("id-{i}"), "v")).collect();
        store.insert_bulk(rows.iter()).await.unwrap();
        assert_eq!(ids(&store).await.len(), n);
    }

    macro_rules! conflict_test {
        (
            $test:ident,
            $ty:ident,
            $table:literal,
            $conflict:ident,
            $suffix:literal,
            $final_a:literal
        ) => {
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
                    const KEY_COLS: &'static [&'static str] = &["id"];
                    const SQL_COLS: &'static str = "id, val";
                    const SQL_KEYS_CSV: &'static str = "id";
                    const SQL_SELECT_ALL: &'static str = concat!("SELECT id, val FROM ", $table);
                    const SQL_SELECT_ALL_ORDERED: &'static str =
                        concat!("SELECT id, val FROM ", $table, " ORDER BY id");
                    const SQL_COUNT: &'static str = concat!("SELECT COUNT(*) FROM ", $table);
                    const SQL_INSERT_PREFIX: &'static str = crate::sqlite::concatcp!(
                        crate::sqlite::insert_verb(Conflict::$conflict),
                        $table,
                        "(id, val) "
                    );
                    const SQL_INSERT_SUFFIX: &'static str = $suffix;
                    const SQL_UPDATE: &'static str =
                        concat!("UPDATE ", $table, " SET val = ? WHERE id = ?");
                    const SQL_WHERE_KEY: &'static str = "id = ?";
                    const SQL_GET: &'static str =
                        concat!("SELECT id, val FROM ", $table, " WHERE id = ?");
                    const SQL_DELETE_BY_KEY: &'static str =
                        concat!("DELETE FROM ", $table, " WHERE id = ?");
                    const SQL_DELETE_ALL: &'static str = concat!("DELETE FROM ", $table);
                    fn bind_row(
                        &self,
                        sep: &mut sqlx::query_builder::Separated<'_, SqliteDb, &'static str>,
                    ) {
                        sep.push_bind(self.id.as_str()).push_bind(self.val.as_str());
                    }
                    fn bind_update<'q>(
                        &'q self,
                        query: sqlx::query::Query<'q, sqlx::Sqlite, sqlx::sqlite::SqliteArguments>,
                    ) -> sqlx::query::Query<'q, sqlx::Sqlite, sqlx::sqlite::SqliteArguments>
                    {
                        query.bind(self.val.as_str()).bind(self.id.as_str())
                    }
                }
                let row = |id: &str, val: &str| $ty {
                    id: id.into(),
                    val: val.into(),
                };

                let sqlite = crate::sqlite::Sqlite::builder().memory().open().await.unwrap();
                sqlx::query(concat!("CREATE TABLE ", $table, "(id TEXT PRIMARY KEY, val TEXT)"))
                    .execute(sqlite.pool())
                    .await
                    .unwrap();
                let view = TableView::<$ty>::new(sqlite);

                view.insert_one(&row("a", "first")).await.unwrap();
                view.insert_bulk([&row("a", "second"), &row("b", "x")]).await.unwrap();

                let count: i64 = sqlx::query_scalar(concat!("SELECT COUNT(*) FROM ", $table))
                    .fetch_one(view.sqlite().pool())
                    .await
                    .unwrap();
                assert_eq!(count, 2, "`b` lands regardless of the conflict on `a`");

                let a: String =
                    sqlx::query_scalar(concat!("SELECT val FROM ", $table, " WHERE id = 'a'"))
                        .fetch_one(view.sqlite().pool())
                        .await
                        .unwrap();
                assert_eq!(a, $final_a);
            }
        };
    }
    conflict_test!(
        upsert_overwrites,
        KvUpsert,
        "c_upsert",
        Upsert,
        " ON CONFLICT (id) DO UPDATE SET val = EXCLUDED.val",
        "second"
    );
    conflict_test!(ignore_keeps_first, KvIgnore, "c_ignore", Ignore, "", "first");
    conflict_test!(replace_overwrites, KvReplace, "c_replace", Replace, "", "second");

    #[tokio::test]
    async fn filter_ordered_and_key_delete() {
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
            const KEY_COLS: &'static [&'static str] = &["host", "id"];
            const SQL_COLS: &'static str = "host, id, val";
            const SQL_KEYS_CSV: &'static str = "host, id";
            const SQL_SELECT_ALL: &'static str = "SELECT host, id, val FROM ev";
            const SQL_SELECT_ALL_ORDERED: &'static str =
                "SELECT host, id, val FROM ev ORDER BY host, id";
            const SQL_COUNT: &'static str = "SELECT COUNT(*) FROM ev";
            const SQL_INSERT_PREFIX: &'static str = "INSERT INTO ev(host, id, val) ";
            const SQL_INSERT_SUFFIX: &'static str =
                " ON CONFLICT (host, id) DO UPDATE SET val = EXCLUDED.val";
            const SQL_UPDATE: &'static str = "UPDATE ev SET val = ? WHERE host = ? AND id = ?";
            const SQL_WHERE_KEY: &'static str = "host = ? AND id = ?";
            const SQL_GET: &'static str = "SELECT host, id, val FROM ev WHERE host = ? AND id = ?";
            const SQL_DELETE_BY_KEY: &'static str = "DELETE FROM ev WHERE host = ? AND id = ?";
            const SQL_DELETE_ALL: &'static str = "DELETE FROM ev";
            fn bind_row(
                &self,
                sep: &mut sqlx::query_builder::Separated<'_, SqliteDb, &'static str>,
            ) {
                sep.push_bind(self.host.as_str())
                    .push_bind(self.id.as_str())
                    .push_bind(self.val.as_str());
            }
            fn bind_update<'q>(
                &'q self,
                query: sqlx::query::Query<'q, sqlx::Sqlite, sqlx::sqlite::SqliteArguments>,
            ) -> sqlx::query::Query<'q, sqlx::Sqlite, sqlx::sqlite::SqliteArguments> {
                // SET val, WHERE host AND id
                query.bind(self.val.as_str()).bind(self.host.as_str()).bind(self.id.as_str())
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
        sqlx::query("CREATE TABLE ev(host TEXT, id TEXT, val TEXT, PRIMARY KEY(host, id))")
            .execute(sqlite.pool())
            .await
            .unwrap();
        let view = TableView::<Ev>::new(sqlite);
        let host = ColRef::<Ev>::new("host");

        // Empty table: both streams are empty.
        assert!(view.all_ordered().try_collect::<Vec<Ev>>().await.unwrap().is_empty());
        assert!(view.filter_by(host, "h1").try_collect::<Vec<Ev>>().await.unwrap().is_empty());

        // insert out of key order to prove the ordering comes from the query.
        view.insert_bulk([&row("h1", "b", "1"), &row("h1", "a", "2"), &row("h2", "a", "3")])
            .await
            .unwrap();

        let all: Vec<(String, String)> =
            view.all_ordered().map_ok(|e| (e.host, e.id)).try_collect().await.unwrap();
        assert_eq!(
            all,
            [("h1", "a"), ("h1", "b"), ("h2", "a")].map(|(h, i)| (h.to_string(), i.to_string()))
        );

        // filter_by the leading key column, ordered by key.
        let h1: Vec<String> =
            view.filter_by(host, "h1").map_ok(|e| e.id).try_collect().await.unwrap();
        assert_eq!(h1, ["a", "b"].map(String::from));

        // A value that matches nothing yields an empty stream.
        assert!(view.filter_by(host, "nope").try_collect::<Vec<Ev>>().await.unwrap().is_empty());

        // The full 2-column key fetches the single row.
        let one = view.get(("h1", "a")).await.unwrap().unwrap();
        assert_eq!(one.val, "2");

        // `delete_by` on the leading key column removes every `h1` row.
        view.delete_by(host, "h1").await.unwrap();
        let hosts: Vec<String> = view.all_ordered().map_ok(|e| e.host).try_collect().await.unwrap();
        assert_eq!(hosts, ["h2".to_string()]);

        // `delete` on a transaction with the full key, committed.
        let mut tx = view.sqlite().pool().begin().await.unwrap();
        view.on(&mut tx).delete(("h2", "a")).await.unwrap();
        tx.commit().await.unwrap();
        assert!(view.all_ordered().try_collect::<Vec<Ev>>().await.unwrap().is_empty());
    }
}
