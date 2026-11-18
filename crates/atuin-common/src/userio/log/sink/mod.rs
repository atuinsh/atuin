//! Where a message for the user ends up.

mod held;
mod nop;
mod stderr;

pub use held::Held;
pub use nop::Nop;
pub use stderr::Stderr;

/// A destination for messages the user must see, such as stderr or a TUI's status line.
///
/// This trait must be implemented by anything that wants to receive
/// [`userio::log::info!`](super::info!), [`userio::log::warn!`](super::warn!), and
/// [`userio::log::error!`](super::error!) messages.
pub trait Sink: Send + Sync {
    /// Show `message` to the user.
    ///
    /// Guarantees that the message display does not interleave and is displayed in the same order
    /// as it was invoked in.
    fn show(&self, message: Message);
}

/// A message for the user.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    pub severity: Severity,
    /// The message without a severity prefix, which each sink renders its own way.
    pub text: String,
    /// The request it answers, if shown while serving one.
    pub request: Option<super::RequestId>,
}

/// How much a message matters to the user.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Info,
    Warn,
    Error,
}
