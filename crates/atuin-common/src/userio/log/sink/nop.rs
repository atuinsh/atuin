//! A sink that drops every message.

use super::{Message, Sink};

/// Drops every message, for a process with nowhere to show one.
pub struct Nop;

impl Sink for Nop {
    fn show(&self, _: Message) {}
}
