//! Conversions between Rust and **Postgres** types.
//!
//! # Types
//!
//! | Rust type                             | Postgres type(s)                                     |
//! |---------------------------------------|------------------------------------------------------|
//! | `bool`                                | BOOL                                                 |
//! | `i8`                                  | "CHAR"                                               |
//! | `i16`                                 | SMALLINT, SMALLSERIAL, INT2                          |
//! | `i32`                                 | INT, SERIAL, INT4                                    |
//! | `i64`                                 | BIGINT, BIGSERIAL, INT8                              |
//! | `f32`                                 | REAL, FLOAT4                                         |
//! | `f64`                                 | DOUBLE PRECISION, FLOAT8                             |
//! | `&str`, [`String`]                    | VARCHAR, CHAR(N), TEXT, NAME                         |
//! | `&[u8]`, `Vec<u8>`                    | BYTEA                                                |
//! | [`PgInterval`]                        | INTERVAL                                             |
//! | [`PgRange<T>`](PgRange)               | INT8RANGE, INT4RANGE, TSRANGE, TSTZTRANGE, DATERANGE, NUMRANGE |
//! | [`PgMoney`]                           | MONEY                                                |
//!
//!
//! ### [`bigdecimal`](https://crates.io/crates/bigdecimal)
//! Requires the `bigdecimal` Cargo feature flag.
//!
//! | Rust type                             | Postgres type(s)                                        |
//! |---------------------------------------|------------------------------------------------------|
//! | `bigdecimal::BigDecimal`              | NUMERIC                                              |
//!
//! ### [`decimal`](https://crates.io/crates/rust_decimal)
//! Requires the `decimal` Cargo feature flag.
//!
//! | Rust type                             | Postgres type(s)                                        |
//! |---------------------------------------|------------------------------------------------------|
//! | `rust_decimal::Decimal`               | NUMERIC                                              |
//!
//! ### [`chrono`](https://crates.io/crates/chrono)
//!
//! Requires the `chrono` Cargo feature flag.
//!
//! | Rust type                             | Postgres type(s)                                     |
//! |---------------------------------------|------------------------------------------------------|
//! | `chrono::DateTime<Utc>`               | TIMESTAMPTZ                                          |
//! | `chrono::DateTime<Local>`             | TIMESTAMPTZ                                          |
//! | `chrono::NaiveDateTime`               | TIMESTAMP                                            |
//! | `chrono::NaiveDate`                   | DATE                                                 |
//! | `chrono::NaiveTime`                   | TIME                                                 |
//! | [`PgTimeTz`]                          | TIMETZ                                               |
//!
//! ### [`time`](https://crates.io/crates/time)
//!
//! Requires the `time` Cargo feature flag.
//!
//! | Rust type                             | Postgres type(s)                                     |
//! |---------------------------------------|------------------------------------------------------|
//! | `time::PrimitiveDateTime`             | TIMESTAMP                                            |
//! | `time::OffsetDateTime`                | TIMESTAMPTZ                                          |
//! | `time::Date`                          | DATE                                                 |
//! | `time::Time`                          | TIME                                                 |
//! | [`PgTimeTz`]                          | TIMETZ                                               |
//!
//! ### [`uuid`](https://crates.io/crates/uuid)
//!
//! Requires the `uuid` Cargo feature flag.
//!
//! | Rust type                             | Postgres type(s)                                     |
//! |---------------------------------------|------------------------------------------------------|
//! | `uuid::Uuid`                          | UUID                                                 |
//!
//! ### [`ipnetwork`](https://crates.io/crates/ipnetwork)
//!
//! Requires the `ipnetwork` Cargo feature flag.
//!
//! | Rust type                             | Postgres type(s)                                     |
//! |---------------------------------------|------------------------------------------------------|
//! | `ipnetwork::IpNetwork`                | INET, CIDR                                           |
//!
//! ### [`bit-vec`](https://crates.io/crates/bit-vec)
//!
//! Requires the `bit-vec` Cargo feature flag.
//!
//! | Rust type                             | Postgres type(s)                                     |
//! |---------------------------------------|------------------------------------------------------|
//! | `bit_vec::BitVec`                     | BIT, VARBIT                                          |
//!
//! ### [`json`](https://crates.io/crates/serde_json)
//!
//! Requires the `json` Cargo feature flag.
//!
//! | Rust type                             | Postgres type(s)                                     |
//! |---------------------------------------|------------------------------------------------------|
//! | [`Json<T>`]                           | JSON, JSONB                                          |
//! | `serde_json::Value`                   | JSON, JSONB                                          |
//! | `&serde_json::value::RawValue`        | JSON, JSONB                                          |
//!
//! `Value` and `RawValue` from `serde_json` can be used for unstructured JSON data with
//! Postgres.
//!
//! [`Json<T>`](crate::types::Json) can be used for structured JSON data with Postgres.
//!
//! # [Composite types](https://www.postgresql.org/docs/current/rowtypes.html)
//!
//! User-defined composite types are supported through a derive for `Type`.
//!
//! ```text
//! CREATE TYPE inventory_item AS (
//!     name            text,
//!     supplier_id     integer,
//!     price           numeric
//! );
//! ```
//!
//! ```rust,ignore
//! #[derive(sqlx::Type)]
//! #[sqlx(type_name = "inventory_item")]
//! struct InventoryItem {
//!     name: String,
//!     supplier_id: i32,
//!     price: BigDecimal,
//! }
//! ```
//!
//! Anonymous composite types are represented as tuples. Note that anonymous composites may only
//! be returned and not sent to Postgres (this is a limitation of postgres).
//!
//! # Arrays
//!
//! One-dimensional arrays are supported as `Vec<T>` or `&[T]` where `T` implements `Type`.
//!
//! # [Enumerations](https://www.postgresql.org/docs/current/datatype-enum.html)
//!
//! User-defined enumerations are supported through a derive for `Type`.
//!
//! ```text
//! CREATE TYPE mood AS ENUM ('sad', 'ok', 'happy');
//! ```
//!
//! ```rust,ignore
//! #[derive(sqlx::Type)]
//! #[sqlx(type_name = "mood", rename_all = "lowercase")]
//! enum Mood { Sad, Ok, Happy }
//! ```
//!
//! Rust enumerations may also be defined to be represented as an integer using `repr`.
//! The following type expects a SQL type of `INTEGER` or `INT4` and will convert to/from the
//! Rust enumeration.
//!
//! ```rust,ignore
//! #[derive(sqlx::Type)]
//! #[repr(i32)]
//! enum Mood { Sad = 0, Ok = 1, Happy = 2 }
//! ```
//!

use crate::postgres::type_info::PgTypeKind;
use crate::postgres::{PgTypeInfo, Postgres};
use crate::types::Type;

mod array;
mod bool;
mod bytes;
mod float;
mod int;
mod interval;
mod money;
mod range;
mod record;
mod str;
mod tuple;
mod void;

#[cfg(any(feature = "chrono", feature = "time"))]
mod time_tz;

#[cfg(feature = "bigdecimal")]
mod bigdecimal;

#[cfg(any(feature = "bigdecimal", feature = "decimal"))]
mod numeric;

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

#[cfg(feature = "ipnetwork")]
mod ipnetwork;

#[cfg(feature = "bit-vec")]
mod bit_vec;

pub use interval::PgInterval;
pub use money::PgMoney;
pub use range::PgRange;

#[cfg(any(feature = "chrono", feature = "time"))]
pub use time_tz::PgTimeTz;

// used in derive(Type) for `struct`
// but the interface is not considered part of the public API
#[doc(hidden)]
pub use record::{PgRecordDecoder, PgRecordEncoder};

// Type::compatible impl appropriate for arrays
fn array_compatible<E: Type<Postgres>>(ty: &PgTypeInfo) -> bool {
    // we require the declared type to be an _array_ with an
    // element type that is acceptable
    if let PgTypeKind::Array(element) = &ty.kind() {
        return E::compatible(&element);
    }

    false
}
