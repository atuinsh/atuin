//! Human-friendly time parser and formatter
//!
//! Features:
//!
//! * Parses durations in free form like `15days 2min 2s`
//! * Formats durations in similar form `2years 2min 12us`
//! * Parses and formats timestamp in `rfc3339` format: `2018-01-01T12:53:00Z`
//! * Parses timestamps in a weaker format: `2018-01-01 12:53:00`
//!
//! Timestamp parsing/formatting is super-fast because format is basically
//! fixed.
//!
//! See [serde-humantime] for serde integration.
//!
//! [serde-humantime]: https://docs.rs/serde-humantime/0.1.1/serde_humantime/
#![warn(missing_debug_implementations)]
#![warn(missing_docs)]

#[macro_use] extern crate quick_error;

mod duration;
mod wrapper;
mod date;

pub use duration::{parse_duration, Error as DurationError};
pub use duration::{format_duration, FormattedDuration};
pub use wrapper::{Duration, Timestamp};
pub use date::{parse_rfc3339, parse_rfc3339_weak, Error as TimestampError};
pub use date::{
    format_rfc3339, format_rfc3339_micros, format_rfc3339_millis, format_rfc3339_nanos,
    format_rfc3339_seconds,
};
pub use date::{Rfc3339Timestamp};
