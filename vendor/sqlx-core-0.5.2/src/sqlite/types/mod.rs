//! Conversions between Rust and **SQLite** types.
//!
//! # Types
//!
//! | Rust type                             | SQLite type(s)                                       |
//! |---------------------------------------|------------------------------------------------------|
//! | `bool`                                | BOOLEAN                                              |
//! | `i8`                                  | INTEGER                                              |
//! | `i16`                                 | INTEGER                                              |
//! | `i32`                                 | INTEGER                                              |
//! | `i64`                                 | BIGINT, INT8                                         |
//! | `u8`                                  | INTEGER                                              |
//! | `u16`                                 | INTEGER                                              |
//! | `u32`                                 | INTEGER                                              |
//! | `u64`                                 | BIGINT, INT8                                         |
//! | `f32`                                 | REAL                                                 |
//! | `f64`                                 | REAL                                                 |
//! | `&str`, [`String`]                    | TEXT                                                 |
//! | `&[u8]`, `Vec<u8>`                    | BLOB                                                 |
//!
//! ### [`chrono`](https://crates.io/crates/chrono)
//!
//! Requires the `chrono` Cargo feature flag.
//!
//! | Rust type                             | Sqlite type(s)                                        |
//! |---------------------------------------|------------------------------------------------------|
//! | `chrono::NaiveDateTime`               | DATETIME                                             |
//! | `chrono::DateTime<Utc>`               | DATETIME                                             |
//! | `chrono::DateTime<Local>`             | DATETIME                                             |
//!
//! ### [`uuid`](https://crates.io/crates/uuid)
//!
//! Requires the `uuid` Cargo feature flag.
//!
//! | Rust type                             | Sqlite type(s)                                       |
//! |---------------------------------------|------------------------------------------------------|
//! | `uuid::Uuid`                          | BLOB, TEXT                                           |
//! | `uuid::adapter::Hyphenated`           | TEXT                                                 |
//!
//! # Nullable
//!
//! In addition, `Option<T>` is supported where `T` implements `Type`. An `Option<T>` represents
//! a potentially `NULL` value from SQLite.
//!

mod bool;
mod bytes;
#[cfg(feature = "chrono")]
mod chrono;
mod float;
mod int;
#[cfg(feature = "json")]
mod json;
mod str;
mod uint;
#[cfg(feature = "uuid")]
mod uuid;
