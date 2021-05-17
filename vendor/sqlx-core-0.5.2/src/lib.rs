//! Core of SQLx, the rust SQL toolkit.
//! Not intended to be used directly.
#![recursion_limit = "512"]
#![warn(future_incompatible, rust_2018_idioms)]
#![allow(clippy::needless_doctest_main, clippy::type_complexity)]
// See `clippy.toml` at the workspace root
#![deny(clippy::disallowed_method)]
//
// Allows an API be documented as only available in some specific platforms.
// <https://doc.rust-lang.org/unstable-book/language-features/doc-cfg.html>
#![cfg_attr(docsrs, feature(doc_cfg))]
//
// When compiling with support for SQLite we must allow some unsafe code in order to
// interface with the inherently unsafe C module. This unsafe code is contained
// to the sqlite module.
#![cfg_attr(feature = "sqlite", deny(unsafe_code))]
#![cfg_attr(not(feature = "sqlite"), forbid(unsafe_code))]

#[cfg(feature = "bigdecimal")]
extern crate bigdecimal_ as bigdecimal;

#[macro_use]
mod ext;

#[macro_use]
pub mod error;

#[macro_use]
pub mod arguments;

#[macro_use]
pub mod pool;

pub mod connection;

#[macro_use]
pub mod transaction;

#[macro_use]
pub mod encode;

#[macro_use]
pub mod decode;

#[macro_use]
pub mod types;

#[macro_use]
pub mod query;

#[macro_use]
pub mod acquire;

#[macro_use]
pub mod column;

#[macro_use]
pub mod statement;

mod common;
pub mod database;
pub mod describe;
pub mod executor;
pub mod from_row;
mod io;
mod logger;
mod net;
pub mod query_as;
pub mod query_scalar;
pub mod row;
pub mod type_info;
pub mod value;

#[cfg(feature = "migrate")]
pub mod migrate;

#[cfg(all(
    any(
        feature = "postgres",
        feature = "mysql",
        feature = "mssql",
        feature = "sqlite"
    ),
    feature = "any"
))]
pub mod any;

#[cfg(feature = "postgres")]
#[cfg_attr(docsrs, doc(cfg(feature = "postgres")))]
pub mod postgres;

#[cfg(feature = "sqlite")]
#[cfg_attr(docsrs, doc(cfg(feature = "sqlite")))]
pub mod sqlite;

#[cfg(feature = "mysql")]
#[cfg_attr(docsrs, doc(cfg(feature = "mysql")))]
pub mod mysql;

#[cfg(feature = "mssql")]
#[cfg_attr(docsrs, doc(cfg(feature = "mssql")))]
pub mod mssql;

/// sqlx uses ahash for increased performance, at the cost of reduced DoS resistance.
use ahash::AHashMap as HashMap;
//type HashMap<K, V> = std::collections::HashMap<K, V, ahash::RandomState>;
