//! A stack of sinks, where the top-most [`Sink`] is the one that receives messages.

use std::sync::Arc;

use parking_lot::Mutex;

use super::{Message, Sink, Stderr};

/// The stack the process's messages go to.
pub static SINK: SinkStack = SinkStack::new();

/// Sinks, the last one pushed and not yet dropped being current; [`Stderr`] when there are none.
#[derive(Default)]
pub struct SinkStack {
    sinks: Mutex<Vec<Arc<dyn Sink>>>,
}

impl SinkStack {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            sinks: Mutex::new(Vec::new()),
        }
    }

    /// Make `sink` the current one until the guard drops.
    pub fn push(&self, sink: impl Sink + 'static) -> SinkGuard<'_> {
        let sink: Arc<dyn Sink> = Arc::new(sink);
        self.sinks.lock().push(Arc::clone(&sink));
        SinkGuard { stack: self, sink }
    }

    /// Show `message` on the current sink.
    pub fn show(&self, message: Message) {
        // Called outside the lock, so a sink stalled on a write never holds up a push or a drop.
        let current = self.sinks.lock().last().cloned();
        match current {
            Some(sink) => sink.show(message),
            None => Stderr::default().show(message),
        }
    }
}

/// Keeps a pushed sink on its stack; dropping it removes the sink wherever it is, in any order.
#[must_use = "the sink is removed as soon as the guard drops"]
pub struct SinkGuard<'a> {
    stack: &'a SinkStack,
    /// The entry to remove after the guard is dropped.
    sink: Arc<dyn Sink>,
}

impl Drop for SinkGuard<'_> {
    fn drop(&mut self) {
        self.stack.sinks.lock().retain(|sink| !Arc::ptr_eq(sink, &self.sink));
    }
}
