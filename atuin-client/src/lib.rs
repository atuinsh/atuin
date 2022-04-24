#![forbid(unsafe_code)]
#![warn(clippy::pedantic, clippy::nursery)]
#![allow(
    clippy::missing_panics_doc,
    clippy::missing_errors_doc,
    clippy::must_use_candidate,
    clippy::unreadable_literal
)]

#[macro_use]
extern crate log;

#[cfg(feature = "sync")]
pub mod api_client;
#[cfg(feature = "sync")]
pub mod encryption;
#[cfg(feature = "sync")]
pub mod sync;

pub mod database;
pub mod history;
pub mod import;
pub mod ordering;
pub mod settings;
