//! # What is Hjson?
//!
//! A configuration file format for humans. Relaxed syntax, fewer mistakes, more comments.
//! See http://hjson.org
//!
//! Data types that can be encoded are JavaScript types (see the `serde_hjson:Value` enum for more
//! details):
//!
//! * `Boolean`: equivalent to rust's `bool`
//! * `I64`: equivalent to rust's `i64`
//! * `U64`: equivalent to rust's `u64`
//! * `F64`: equivalent to rust's `f64`
//! * `String`: equivalent to rust's `String`
//! * `Array`: equivalent to rust's `Vec<T>`, but also allowing objects of different types in the
//!    same array
//! * `Object`: equivalent to rust's `serde_hjson::Map<String, serde_hjson::Value>`
//! * `Null`
//!
//!
//! # Examples of use
//!
//! ## Parsing a `str` to `Value` and reading the result
//!
//! ```rust
//! //#![feature(custom_derive, plugin)]
//! //#![plugin(serde_macros)]
//!
//! extern crate serde_hjson;
//!
//! use serde_hjson::Value;
//!
//! fn main() {
//!     let data: Value = serde_hjson::from_str("{foo: 13, bar: \"baz\"}").unwrap();
//!     println!("data: {:?}", data);
//!     println!("object? {}", data.is_object());
//!
//!     let obj = data.as_object().unwrap();
//!     let foo = obj.get("foo").unwrap();
//!
//!     println!("array? {:?}", foo.as_array());
//!     // array? None
//!     println!("u64? {:?}", foo.as_u64());
//!     // u64? Some(13u64)
//!
//!     for (key, value) in obj.iter() {
//!         println!("{}: {}", key, match *value {
//!             Value::U64(v) => format!("{} (u64)", v),
//!             Value::String(ref v) => format!("{} (string)", v),
//!             _ => unreachable!(),
//!         });
//!     }
//!     // bar: baz (string)
//!     // foo: 13 (u64)
//! }
//! ```

#![cfg_attr(feature = "nightly-testing", plugin(clippy))]
#![deny(missing_docs)]

#[macro_use] extern crate lazy_static;

#[cfg(feature = "preserve_order")]
extern crate linked_hash_map;
extern crate core;
extern crate num_traits;
extern crate regex;
extern crate serde;

pub use self::de::{
    Deserializer,
    StreamDeserializer,
    from_iter,
    from_reader,
    from_slice,
    from_str,
};
pub use self::error::{Error, ErrorCode, Result};
pub use self::ser::{
    Serializer,
    to_writer,
    to_vec,
    to_string,
};
pub use self::value::{Value, Map, to_value, from_value};

#[macro_use]
mod forward;

pub mod builder;
pub mod de;
pub mod error;
pub mod ser;
mod util;
pub mod value;
