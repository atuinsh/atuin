//! Utilities for immediately-visible logs.
//!
//! Unlike [`tracing`] logs, these utilities are designed for messages that the user must
//! immediately see.
//!
//! The utility is designed around the concept of a "mutable" sink. You are expected to configure it
//! with the [`Sink`] trait.
//!
//! Implementing that trait enables you to figure out how messages get displayed to the user. You
//! are expected to use the thread-safe `SINK` utility to change the active sink.
//!
//! # Implementation
//!
//! Under the hood, this is implemented through [`tracing`] and every wrapped here--[`info!`],
//! [`warn!`], and [`error!`] in reality emit [`tracing`] events that custom layers handle.
mod sink;

pub use sink::{Message, Severity, Sink, Stderr};

/// Target for messages meant for the user, as opposed to diagnostics.
pub const USER: &str = "atuin-user";

#[doc(hidden)]
pub use tracing as __tracing;

/// [`tracing::info!`] for a message the user reads, on the [`USER`] target.
#[macro_export]
macro_rules! __atuin_log_info {
    ($($arg:tt)+) => {
        $crate::userio::log::__tracing::info!(target: $crate::userio::log::USER, $($arg)+)
    };
}

/// [`tracing::warn!`] for a message the user reads, on the [`USER`] target.
#[macro_export]
macro_rules! __atuin_log_warn {
    ($($arg:tt)+) => {
        $crate::userio::log::__tracing::warn!(target: $crate::userio::log::USER, $($arg)+)
    };
}

/// [`tracing::error!`] for a message the user reads, on the [`USER`] target.
#[macro_export]
macro_rules! __atuin_log_error {
    ($($arg:tt)+) => {
        $crate::userio::log::__tracing::error!(target: $crate::userio::log::USER, $($arg)+)
    };
}

pub use __atuin_log_error as error;
pub use __atuin_log_info as info;
pub use __atuin_log_warn as warn;

/// [`info!`] with a message from the calling crate's catalogue: `info!("message-id", name = value)`.
///
/// The calling crate must provide `crate::i18n::fl!`, which fails the build on an unknown id or
/// mismatched arguments. The id is also recorded as the `message_id` field.
#[macro_export]
#[expect(clippy::crate_in_macro_def, reason = "fl! must resolve to the calling crate's catalogue")]
macro_rules! __atuin_log_localized_info {
    ($message_id:literal $(, $($args:tt)+)?) => {
        $crate::userio::log::__tracing::info!(
            target: $crate::userio::log::USER,
            message_id = $message_id,
            "{}",
            crate::i18n::fl!($message_id $(, $($args)+)?)
        )
    };
}

/// [`warn!`] with a message from the calling crate's catalogue; see [`localized::info!`].
#[macro_export]
#[expect(clippy::crate_in_macro_def, reason = "fl! must resolve to the calling crate's catalogue")]
macro_rules! __atuin_log_localized_warn {
    ($message_id:literal $(, $($args:tt)+)?) => {
        $crate::userio::log::__tracing::warn!(
            target: $crate::userio::log::USER,
            message_id = $message_id,
            "{}",
            crate::i18n::fl!($message_id $(, $($args)+)?)
        )
    };
}

/// [`error!`] with a message from the calling crate's catalogue; see [`localized::info!`].
#[macro_export]
#[expect(clippy::crate_in_macro_def, reason = "fl! must resolve to the calling crate's catalogue")]
macro_rules! __atuin_log_localized_error {
    ($message_id:literal $(, $($args:tt)+)?) => {
        $crate::userio::log::__tracing::error!(
            target: $crate::userio::log::USER,
            message_id = $message_id,
            "{}",
            crate::i18n::fl!($message_id $(, $($args)+)?)
        )
    };
}

/// Localized [`info!`], [`warn!`] and [`error!`], which take their place once every call site is
/// ported.
pub mod localized {
    pub use crate::{
        __atuin_log_localized_error as error, __atuin_log_localized_info as info,
        __atuin_log_localized_warn as warn,
    };
}
