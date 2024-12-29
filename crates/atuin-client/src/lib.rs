#![forbid(unsafe_code)]

#[macro_use]
extern crate rust_i18n;

#[macro_use]
extern crate log;

#[cfg(feature = "sync")]
pub mod api_client;
#[cfg(feature = "sync")]
pub mod sync;

pub mod database;
pub mod encryption;
pub mod history;
pub mod import;
pub mod kv;
pub mod login;
pub mod logout;
pub mod ordering;
pub mod record;
pub mod register;
pub mod secrets;
pub mod settings;
pub mod theme;

mod utils;

i18n!("locales", fallback = "en");

pub fn set_locale(locale: &str) {
    rust_i18n::set_locale(locale)
}
