//! Utilities for immediately-visible logs.
//!
//! Unlike [`tracing`] logs, these utilities are designed for messages that the user must
//! immediately see.
//!
//! The utility is designed around the concept of a "mutable" sink. You are expected to configure it
//! with the [`Sink`] trait.
//!
//! Implementing that trait enables you to figure out how messages get displayed to the user. You
//! are expected to use the thread-safe [`SINK`] utility to change the active sink.
//!
//! # Implementation
//!
//! Under the hood, this is implemented through [`tracing`] and every wrapped here--[`info!`],
//! [`warn!`], and [`error!`] in reality emit [`tracing`] events that custom layers handle. A process
//! shows them by adding a [`UserLayer`] to its subscriber.
mod layer;
mod request;
mod sink;
mod stack;

pub use layer::{UserFilterExt, UserLayer};
pub use request::RequestId;
pub use sink::{Held, Message, Nop, Severity, Sink, Stderr};
pub use stack::{SINK, SinkGuard, SinkStack};

/// Target for messages meant for the user, as opposed to diagnostics.
pub const USER: &str = "atuin-user";

#[doc(hidden)]
pub use tracing as __tracing;

// Users may write `userio::log::info!("{}", side_effect())`. Without info traces, side-effects
// would get disabled, which we never want. Compiling the program without info-level logs is,
// therefore, banned.
const _: () = assert!(
    matches!(
        tracing::level_filters::STATIC_MAX_LEVEL,
        tracing::level_filters::LevelFilter::INFO
            | tracing::level_filters::LevelFilter::DEBUG
            | tracing::level_filters::LevelFilter::TRACE
    ),
    "userio::log's info!, warn! and error! must not be compiled out by `max_level_*` features"
);

/// [`tracing::info!`] for a message from the calling crate's catalogue, on the [`USER`] target:
/// `info!("message-id", name = value)`.
///
/// TODO(markovejnovic): At some point we should consolidate all the catalogues.
#[macro_export]
#[expect(clippy::crate_in_macro_def, reason = "the message is in the calling crate's catalogue")]
macro_rules! __atuin_log_info {
    ($message_id:literal $(, $($args:tt)+)?) => {
        $crate::userio::log::__tracing::info!(
            target: $crate::userio::log::USER,
            "{}",
            $crate::i18n::fl!(crate::i18n::LOADER; $message_id $(, $($args)+)?)
        )
    };
}

/// [`tracing::warn!`] for a message from the calling crate's catalogue; see [`info!`].
#[macro_export]
#[expect(clippy::crate_in_macro_def, reason = "the message is in the calling crate's catalogue")]
macro_rules! __atuin_log_warn {
    ($message_id:literal $(, $($args:tt)+)?) => {
        $crate::userio::log::__tracing::warn!(
            target: $crate::userio::log::USER,
            "{}",
            $crate::i18n::fl!(crate::i18n::LOADER; $message_id $(, $($args)+)?)
        )
    };
}

/// [`tracing::error!`] for a message from the calling crate's catalogue; see [`info!`].
#[macro_export]
#[expect(clippy::crate_in_macro_def, reason = "the message is in the calling crate's catalogue")]
macro_rules! __atuin_log_error {
    ($message_id:literal $(, $($args:tt)+)?) => {
        $crate::userio::log::__tracing::error!(
            target: $crate::userio::log::USER,
            "{}",
            $crate::i18n::fl!(crate::i18n::LOADER; $message_id $(, $($args)+)?)
        )
    };
}

pub use __atuin_log_error as error;
pub use __atuin_log_info as info;
pub use __atuin_log_warn as warn;
