# Rusqlite

[![Travis Build Status](https://api.travis-ci.org/rusqlite/rusqlite.svg?branch=master)](https://travis-ci.org/rusqlite/rusqlite)
[![AppVeyor Build Status](https://ci.appveyor.com/api/projects/status/github/rusqlite/rusqlite?branch=master&svg=true)](https://ci.appveyor.com/project/rusqlite/rusqlite)
[![Build Status](https://github.com/rusqlite/rusqlite/workflows/CI/badge.svg)](https://github.com/rusqlite/rusqlite/actions)
[![dependency status](https://deps.rs/repo/github/rusqlite/rusqlite/status.svg)](https://deps.rs/repo/github/rusqlite/rusqlite)
[![Latest Version](https://img.shields.io/crates/v/rusqlite.svg)](https://crates.io/crates/rusqlite)
[![Gitter](https://badges.gitter.im/rusqlite.svg)](https://gitter.im/rusqlite/community?utm_source=badge&utm_medium=badge&utm_campaign=pr-badge)
[![Docs](https://docs.rs/rusqlite/badge.svg)](https://docs.rs/rusqlite)
[![codecov](https://codecov.io/gh/rusqlite/rusqlite/branch/master/graph/badge.svg)](https://codecov.io/gh/rusqlite/rusqlite)

Rusqlite is an ergonomic wrapper for using SQLite from Rust. It attempts to expose
an interface similar to [rust-postgres](https://github.com/sfackler/rust-postgres).

```rust
use rusqlite::{params, Connection, Result};

#[derive(Debug)]
struct Person {
    id: i32,
    name: String,
    data: Option<Vec<u8>>,
}

fn main() -> Result<()> {
    let conn = Connection::open_in_memory()?;

    conn.execute(
        "CREATE TABLE person (
                  id              INTEGER PRIMARY KEY,
                  name            TEXT NOT NULL,
                  data            BLOB
                  )",
        [],
    )?;
    let me = Person {
        id: 0,
        name: "Steven".to_string(),
        data: None,
    };
    conn.execute(
        "INSERT INTO person (name, data) VALUES (?1, ?2)",
        params![me.name, me.data],
    )?;

    let mut stmt = conn.prepare("SELECT id, name, data FROM person")?;
    let person_iter = stmt.query_map([], |row| {
        Ok(Person {
            id: row.get(0)?,
            name: row.get(1)?,
            data: row.get(2)?,
        })
    })?;

    for person in person_iter {
        println!("Found person {:?}", person.unwrap());
    }
    Ok(())
}
```

### Supported SQLite Versions

The base `rusqlite` package supports SQLite version 3.6.8 or newer. If you need
support for older versions, please file an issue. Some cargo features require a
newer SQLite version; see details below.

### Optional Features

Rusqlite provides several features that are behind [Cargo
features](https://doc.rust-lang.org/cargo/reference/manifest.html#the-features-section). They are:

* [`load_extension`](https://docs.rs/rusqlite/~0/rusqlite/struct.LoadExtensionGuard.html)
  allows loading dynamic library-based SQLite extensions.
* [`backup`](https://docs.rs/rusqlite/~0/rusqlite/backup/index.html)
  allows use of SQLite's online backup API. Note: This feature requires SQLite 3.6.11 or later.
* [`functions`](https://docs.rs/rusqlite/~0/rusqlite/functions/index.html)
  allows you to load Rust closures into SQLite connections for use in queries.
  Note: This feature requires SQLite 3.7.3 or later.
* `window` for [window function](https://www.sqlite.org/windowfunctions.html) support (`fun(...) OVER ...`). (Implies `functions`.)
* [`trace`](https://docs.rs/rusqlite/~0/rusqlite/trace/index.html)
  allows hooks into SQLite's tracing and profiling APIs. Note: This feature
  requires SQLite 3.6.23 or later.
* [`blob`](https://docs.rs/rusqlite/~0/rusqlite/blob/index.html)
  gives `std::io::{Read, Write, Seek}` access to SQL BLOBs. Note: This feature
  requires SQLite 3.7.4 or later.
* [`limits`](https://docs.rs/rusqlite/~0/rusqlite/struct.Connection.html#method.limit)
  allows you to set and retrieve SQLite's per connection limits.
* `chrono` implements [`FromSql`](https://docs.rs/rusqlite/~0/rusqlite/types/trait.FromSql.html)
  and [`ToSql`](https://docs.rs/rusqlite/~0/rusqlite/types/trait.ToSql.html) for various
  types from the [`chrono` crate](https://crates.io/crates/chrono).
* `serde_json` implements [`FromSql`](https://docs.rs/rusqlite/~0/rusqlite/types/trait.FromSql.html)
  and [`ToSql`](https://docs.rs/rusqlite/~0/rusqlite/types/trait.ToSql.html) for the
  `Value` type from the [`serde_json` crate](https://crates.io/crates/serde_json).
* `time` implements [`FromSql`](https://docs.rs/rusqlite/~0/rusqlite/types/trait.FromSql.html)
   and [`ToSql`](https://docs.rs/rusqlite/~0/rusqlite/types/trait.ToSql.html) for the
   `time::OffsetDateTime` type from the [`time` crate](https://crates.io/crates/time).
* `url` implements [`FromSql`](https://docs.rs/rusqlite/~0/rusqlite/types/trait.FromSql.html)
  and [`ToSql`](https://docs.rs/rusqlite/~0/rusqlite/types/trait.ToSql.html) for the
  `Url` type from the [`url` crate](https://crates.io/crates/url).
* `bundled` uses a bundled version of SQLite.  This is a good option for cases where linking to SQLite is complicated, such as Windows.
* `sqlcipher` looks for the SQLCipher library to link against instead of SQLite. This feature is mutually exclusive with `bundled`.
* `hooks` for [Commit, Rollback](http://sqlite.org/c3ref/commit_hook.html) and [Data Change](http://sqlite.org/c3ref/update_hook.html) notification callbacks.
* `unlock_notify` for [Unlock](https://sqlite.org/unlock_notify.html) notification.
* `vtab` for [virtual table](https://sqlite.org/vtab.html) support (allows you to write virtual table implementations in Rust). Currently, only read-only virtual tables are supported.
* `series` exposes [`generate_series(...)`](https://www.sqlite.org/series.html) Table-Valued Function. (Implies `vtab`.)
* [`csvtab`](https://sqlite.org/csv.html), CSV virtual table written in Rust. (Implies `vtab`.)
* [`array`](https://sqlite.org/carray.html), The `rarray()` Table-Valued Function. (Implies `vtab`.)
* `i128_blob` allows storing values of type `i128` type in SQLite databases. Internally, the data is stored as a 16 byte big-endian blob, with the most significant bit flipped, which allows ordering and comparison between different blobs storing i128s to work as expected.
* `uuid` allows storing and retrieving `Uuid` values from the [`uuid`](https://docs.rs/uuid/) crate using blobs.
* [`session`](https://sqlite.org/sessionintro.html), Session module extension. Requires `buildtime_bindgen` feature. (Implies `hooks`.)
* `extra_check` fail when a query passed to execute is readonly or has a column count > 0.
* `column_decltype` provides `columns()` method for Statements and Rows; omit if linking to a version of SQLite/SQLCipher compiled with `-DSQLITE_OMIT_DECLTYPE`.
* `collation` exposes [`sqlite3_create_collation_v2`](https://sqlite.org/c3ref/create_collation.html).

## Notes on building rusqlite and libsqlite3-sys

`libsqlite3-sys` is a separate crate from `rusqlite` that provides the Rust
declarations for SQLite's C API. By default, `libsqlite3-sys` attempts to find a SQLite library that already exists on your system using pkg-config, or a
[Vcpkg](https://github.com/Microsoft/vcpkg) installation for MSVC ABI builds.

You can adjust this behavior in a number of ways:

* If you use the `bundled` feature, `libsqlite3-sys` will use the
  [cc](https://crates.io/crates/cc) crate to compile SQLite from source and
  link against that. This source is embedded in the `libsqlite3-sys` crate and
  is currently SQLite 3.35.4 (as of `rusqlite` 0.25.0 / `libsqlite3-sys`
  0.22.0).  This is probably the simplest solution to any build problems. You can enable this by adding the following in your `Cargo.toml` file:
  ```toml
  [dependencies.rusqlite]
  version = "0.25.1"
  features = ["bundled"]
  ```
* When using the `bundled` feature, the build script will honor `SQLITE_MAX_VARIABLE_NUMBER` and `SQLITE_MAX_EXPR_DEPTH` variables. It will also honor a `LIBSQLITE3_FLAGS` variable, which can have a format like `"-USQLITE_ALPHA -DSQLITE_BETA SQLITE_GAMMA ..."`. That would disable the `SQLITE_ALPHA` flag, and set the `SQLITE_BETA` and `SQLITE_GAMMA` flags. (The initial `-D` can be omitted, as on the last one.)

* When linking against a SQLite library already on the system (so *not* using the `bundled` feature), you can set the `SQLITE3_LIB_DIR` environment variable to point to a directory containing the library. You can also set the `SQLITE3_INCLUDE_DIR` variable to point to the directory containing `sqlite3.h`.
* Installing the sqlite3 development packages will usually be all that is required, but
  the build helpers for [pkg-config](https://github.com/alexcrichton/pkg-config-rs)
  and [vcpkg](https://github.com/mcgoo/vcpkg-rs) have some additional configuration
  options. The default when using vcpkg is to dynamically link,
  which must be enabled by setting `VCPKGRS_DYNAMIC=1` environment variable before build.
  `vcpkg install sqlite3:x64-windows` will install the required library.
* When linking against a SQLite library already on the system, you can set the `SQLITE3_STATIC` environment variable to 1 to request that the library be statically instead of dynamically linked.


### Binding generation

We use [bindgen](https://crates.io/crates/bindgen) to generate the Rust
declarations from SQLite's C header file. `bindgen`
[recommends](https://github.com/servo/rust-bindgen#library-usage-with-buildrs)
running this as part of the build process of libraries that used this. We tried
this briefly (`rusqlite` 0.10.0, specifically), but it had some annoyances:

* The build time for `libsqlite3-sys` (and therefore `rusqlite`) increased
  dramatically.
* Running `bindgen` requires a relatively-recent version of Clang, which many
  systems do not have installed by default.
* Running `bindgen` also requires the SQLite header file to be present.

As of `rusqlite` 0.10.1, we avoid running `bindgen` at build-time by shipping
pregenerated bindings for several versions of SQLite. When compiling
`rusqlite`, we use your selected Cargo features to pick the bindings for the
minimum SQLite version that supports your chosen features. If you are using
`libsqlite3-sys` directly, you can use the same features to choose which
pregenerated bindings are chosen:

* `min_sqlite_version_3_6_8` - SQLite 3.6.8 bindings (this is the default)
* `min_sqlite_version_3_6_23` - SQLite 3.6.23 bindings
* `min_sqlite_version_3_7_7` - SQLite 3.7.7 bindings

If you use the `bundled` feature, you will get pregenerated bindings for the
bundled version of SQLite. If you need other specific pregenerated binding
versions, please file an issue. If you want to run `bindgen` at buildtime to
produce your own bindings, use the `buildtime_bindgen` Cargo feature.

If you enable the `modern_sqlite` feature, we'll use the bindings we would have
included with the bundled build. You generally should have `buildtime_bindgen`
enabled if you turn this on, as otherwise you'll need to keep the version of
SQLite you link with in sync with what rusqlite would have bundled, (usually the
most recent release of SQLite). Failing to do this will cause a runtime error.

## Contributing

Rusqlite has many features, and many of them impact the build configuration in
incompatible ways. This is unfortunate, and makes testing changes hard.

To help here: you generally should ensure that you run tests/lint for
`--features bundled`, and `--features "bundled-full session buildtime_bindgen"`.

If running bindgen is problematic for you, `--features bundled-full` enables
bundled and all features which don't require binding generation, and can be used
instead.

### Checklist

- Run `cargo fmt` to ensure your Rust code is correctly formatted.
- Ensure `cargo clippy --all-targets --workspace --features bundled` passes without warnings.
- Ensure `cargo clippy --all-targets --workspace --features "bundled-full session buildtime_bindgen"` passes without warnings.
- Ensure `cargo test --all-targets --workspace --features bundled` reports no failures.
- Ensure `cargo test --all-targets --workspace --features "bundled-full session buildtime_bindgen"` reports no failures.

## Author

Rusqlite is the product of hard work by a number of people. A list is available
here: https://github.com/rusqlite/rusqlite/graphs/contributors

## Community

Currently there's a gitter channel set up for rusqlite [here](https://gitter.im/rusqlite/community).

## License

Rusqlite is available under the MIT license. See the LICENSE file for more info.
