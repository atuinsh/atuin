#![deny(unsafe_code)]

#[macro_use]
extern crate log;

#[cfg(feature = "sync")]
pub mod api_client;
#[cfg(feature = "hub")]
pub mod hub;
#[cfg(feature = "sync")]
pub mod login;
#[cfg(feature = "sync")]
pub mod register;
#[cfg(feature = "sync")]
pub mod sync;

pub mod database;
pub mod encryption;
pub mod history;
pub mod import;
pub mod logout;
pub mod meta;
pub mod ordering;
pub mod plugin;
pub mod record;
pub mod secrets;
pub mod settings;
pub mod theme;

mod utils;
