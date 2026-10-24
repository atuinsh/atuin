#![deny(unsafe_code)]
#![cfg_attr(test, allow(clippy::disallowed_macros, reason = "tests may print"))]

pub mod hub;
pub mod pb;
pub mod queue;
