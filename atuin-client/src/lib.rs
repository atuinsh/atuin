#![forbid(unsafe_code)]

#[macro_use]
extern crate log;

#[macro_use]
extern crate serde_derive;

#[cfg(feature = "sync")]
pub mod api_client;
pub mod database;
#[cfg(feature = "sync")]
pub mod encryption;
pub mod history;
pub mod import;
pub mod ordering;
pub mod settings;
#[cfg(feature = "sync")]
pub mod sync;
