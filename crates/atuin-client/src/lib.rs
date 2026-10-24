#![deny(unsafe_code)]
#![cfg_attr(test, allow(clippy::disallowed_macros, reason = "tests may print"))]

#[macro_use]
extern crate tracing;

#[cfg(feature = "sync")]
pub mod api_client;
#[cfg(feature = "sync")]
pub mod auth;
#[cfg(feature = "hub")]
pub mod hub;

pub mod ai_session;
pub mod database;
pub mod distro;
pub mod history;
pub mod import;
pub mod logout;
pub mod logs;
pub mod meta;
pub mod ordering;
pub mod packfile;
pub mod plugin;
pub mod record;
pub mod settings;
pub mod theme;
pub mod tui;
