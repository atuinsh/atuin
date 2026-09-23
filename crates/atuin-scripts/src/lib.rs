#![cfg_attr(test, allow(clippy::disallowed_methods, reason = "tests may use std::fs for fixtures"))]
// TODO(markovejnovic): remove once atuin-scripts is migrated (fd-pool migration)
#![cfg_attr(not(test), allow(clippy::disallowed_methods))]

pub mod database;
pub mod execution;
pub mod settings;
pub mod store;
