//! A sink that keeps every message for its owner to show.

use std::sync::Arc;

use parking_lot::Mutex;

use super::{Message, Sink};

/// Keeps every message until it is [taken](Self::take), for a process that shows them its own
/// way, such as a TUI drawn where stderr would be.
#[derive(Clone, Default)]
pub struct Held(Arc<Mutex<Vec<Message>>>);

impl Held {
    /// The messages held since the last take, oldest first.
    #[must_use]
    pub fn take(&self) -> Vec<Message> {
        std::mem::take(&mut self.0.lock())
    }
}

impl Sink for Held {
    fn show(&self, message: Message) {
        self.0.lock().push(message);
    }
}
