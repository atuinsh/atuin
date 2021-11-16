#![forbid(unsafe_code)]

#[cfg(feature = "sync")]
#[macro_use]
extern crate serde_derive;

#[cfg(feature = "sync")]
pub mod api;
pub mod utils;
