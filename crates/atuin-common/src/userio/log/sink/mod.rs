//! Where a message for the user ends up.

mod channel;
mod nop;
mod stderr;

pub use channel::Channel;
pub use nop::Nop;
pub use stderr::Stderr;

/// A destination for messages the user must see, such as stderr or a TUI's status line.
///
/// Only the binary picks the sink; library code never does.
pub trait Sink: Send + Sync {
    /// Show `message` to the user.
    ///
    /// Guarantees that the message diplay do not interleave and is displayed in the same order as
    /// it was invoked in.
    fn show(&self, message: Message);
}

/// A message for the user.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    pub severity: Severity,
    /// The message without a severity prefix, which each sink renders its own way.
    pub text: String,
}

/// How much a message matters to the user.
///
/// `Error` is the greatest, unlike [`tracing::Level`], which orders by verbosity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Info,
    Warn,
    Error,
}
