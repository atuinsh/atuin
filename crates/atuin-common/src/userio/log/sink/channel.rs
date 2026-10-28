//! A sink that hands every message to a receiver.

use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};

use super::{Message, Sink};

/// Sends every message to its receiver, for a TUI to show while it owns the terminal.
///
/// The receiver closes once the sink is dropped, so draining it after the sink's guard drops
/// misses nothing.
pub struct Channel {
    sender: UnboundedSender<Message>,
}

impl Channel {
    #[must_use]
    pub fn new() -> (Self, UnboundedReceiver<Message>) {
        let (sender, receiver) = mpsc::unbounded_channel();
        (Self { sender }, receiver)
    }
}

impl Sink for Channel {
    fn show(&self, message: Message) {
        if let Err(unsent) = self.sender.send(message) {
            tracing::warn!(text = %unsent.0.text, "dropped a message for the user: no receiver");
        }
    }
}
