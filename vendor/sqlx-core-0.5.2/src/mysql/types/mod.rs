//! Conversions between Rust and **MySQL** types.
//!
//! # Types
//!
//! | Rust type                             | MySQL type(s)                                        |
//! |---------------------------------------|------------------------------------------------------|
//! | `bool`                                | TINYINT(1), BOOLEAN                                  |
//! | `i8`                                  | TINYINT                                              |
//! | `i16`                                 | SMALLINT                                             |
//! | `i32`                                 | INT                                                  |
//! | `i64`                                 | BIGINT                                               |
//! | `u8`                                  | TINYINT UNSIGNED                                     |
//! | `u16`                                 | SMALLINT UNSIGNED                                    |
//! | `u32`                                 | INT UNSIGNED                                         |
//! | `u64`                                 | BIGINT UNSIGNED                                      |
//! | `f32`                                 | FLOAT                                                |
//! | `f64`                                 | DOUBLE                                               |
//! | `&str`, [`String`]                    | VARCHAR, CHAR, TEXT                                  |
//! | `&[u8]`, `Vec<u8>`                    | VARBINARY, BINARY, BLOB                              |
//!
//! ### [`chrono`](https://crates.io/crates/chrono)
//!
//! Requires the `chrono` Cargo feature flag.
//!
//! | Rust type                             | MySQL type(s)                                        |
//! |---------------------------------------|------------------------------------------------------|
//! | `chrono::DateTime<Utc>`               | TIMESTAMP                                            |
//! | `chrono::DateTime<Local>`             | TIMESTAMP                                            |
//! | `chrono::NaiveDateTime`               | DATETIME                                             |
//! | `chrono::NaiveDate`                   | DATE                                                 |
//! | `chrono::NaiveTime`                   | TIME                                                 |
//!
//! ### [`time`](https://crates.io/crates/time)
//!
//! Requires the `time` Cargo feature flag.
//!
//! | Rust type                             | MySQL type(s)                                        |
//! |---------------------------------------|------------------------------------------------------|
//! | `time::PrimitiveDateTime`             | DATETIME                                             |
//! | `time::OffsetDateTime`                | TIMESTAMP                                            |
//! | `time::Date`                          | DATE                                                 |
//! | `time::Time`                          | TIME                                                 |
//!
//! ### [`bigdecimal`](https://crates.io/crates/bigdecimal)
//! Requires the `bigdecimal` Cargo feature flag.
//!
//! | Rust type                             | MySQL type(s)                                        |
//! |---------------------------------------|------------------------------------------------------|
//! | `bigdecimal::BigDecimal`              | DECIMAL                                              |
//!
//! ### [`decimal`](https://crates.io/crates/rust_decimal)
//! Requires the `decimal` Cargo feature flag.
//!
//! | Rust type                             | MySQL type(s)                                        |
//! |---------------------------------------|------------------------------------------------------|
//! | `rust_decimal::Decimal`               | DECIMAL                                              |
//!
//! ### [`uuid`](https://crates.io/crates/uuid)
//!
//! Requires the `uuid` Cargo feature flag.
//!
//! | Rust type                             | MySQL type(s)                                        |
//! |---------------------------------------|------------------------------------------------------|
//! | `uuid::Uuid`                          | BYTE(16), VARCHAR, CHAR, TEXT                        |
//! | `uuid::adapter::Hyphenated`           | CHAR(36)                                             |
//!
//! ### [`json`](https://crates.io/crates/json)
//!
//! Requires the `json` Cargo feature flag.
//!
//! | Rust type                             | MySQL type(s)                                        |
//! |---------------------------------------|------------------------------------------------------|
//! | `json::JsonValue`             | JSON
//!
//! # Nullable
//!
//! In addition, `Option<T>` is supported where `T` implements `Type`. An `Option<T>` represents
//! a potentially `NULL` value from MySQL.
//!

mod bool;
mod bytes;
mod float;
mod int;
mod str;
mod uint;

#[cfg(feature = "bigdecimal")]
mod bigdecimal;

#[cfg(feature = "decimal")]
mod decimal;

#[cfg(feature = "chrono")]
mod chrono;

#[cfg(feature = "time")]
mod time;

#[cfg(feature = "uuid")]
mod uuid;

#[cfg(feature = "json")]
mod json;
