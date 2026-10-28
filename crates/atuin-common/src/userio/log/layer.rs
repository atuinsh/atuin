//! The [`tracing`] layer that shows [`USER`] events on a [`SinkStack`].

use std::fmt;

use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::layer::{Context, Layer};

use super::{Message, Severity, SinkStack, USER};

/// Shows every [`USER`] event on the current sink of a [`SinkStack`].
pub struct UserLayer {
    stack: &'static SinkStack,
}

impl UserLayer {
    #[must_use]
    pub const fn new(stack: &'static SinkStack) -> Self {
        Self { stack }
    }
}

impl<S: Subscriber> Layer<S> for UserLayer {
    fn on_event(&self, event: &Event<'_>, _: Context<'_, S>) {
        let metadata = event.metadata();
        // Filtered here rather than in `enabled`, which would filter every layer's events.
        if metadata.target() != USER {
            return;
        }
        let Some(severity) = Severity::from_level(*metadata.level()) else {
            return;
        };
        let mut text = Text(None);
        event.record(&mut text);
        let Some(text) = text.0 else {
            return;
        };
        self.stack.show(Message { severity, text });
    }
}

impl Severity {
    /// The severity of an event at `level`, if users see that level.
    const fn from_level(level: Level) -> Option<Self> {
        match level {
            Level::ERROR => Some(Self::Error),
            Level::WARN => Some(Self::Warn),
            Level::INFO => Some(Self::Info),
            _ => None,
        }
    }
}

/// An event's `message` field.
struct Text(Option<String>);

impl Visit for Text {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.0 = Some(value.to_owned());
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        if field.name() == "message" {
            self.0 = Some(format!("{value:?}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use rstest::{fixture, rstest};
    use tokio::sync::mpsc::error::TryRecvError;
    use tracing_subscriber::layer::SubscriberExt as _;

    use super::*;
    use crate::userio::log::{self, Channel};

    #[fixture]
    fn stack() -> &'static SinkStack {
        Box::leak(Box::default())
    }

    fn emit(stack: &'static SinkStack, events: fn()) {
        let subscriber = tracing_subscriber::registry().with(UserLayer::new(stack));
        tracing::subscriber::with_default(subscriber, events);
    }

    #[rstest]
    #[case::info(|| log::info!("synced {} records", 3), Severity::Info, "synced 3 records")]
    #[case::warn(|| log::warn!("no key"), Severity::Warn, "no key")]
    #[case::error(|| log::error!("sync failed"), Severity::Error, "sync failed")]
    #[case::fields_left_out(
        || log::info!(message_id = "sync-up-down", "1/2 up/down"),
        Severity::Info,
        "1/2 up/down"
    )]
    fn shows_user_events(
        stack: &'static SinkStack,
        #[case] events: fn(),
        #[case] severity: Severity,
        #[case] text: &str,
    ) {
        let (sink, mut messages) = Channel::new();
        let _sink = stack.push(sink);
        emit(stack, events);
        assert_eq!(
            messages.try_recv(),
            Ok(Message {
                severity,
                text: text.to_owned()
            })
        );
    }

    #[rstest]
    #[case::diagnostic(|| tracing::warn!("index rebuilt"))]
    #[case::debug_on_user(|| tracing::debug!(target: USER, "too quiet to show"))]
    fn ignores_other_events(stack: &'static SinkStack, #[case] events: fn()) {
        let (sink, mut messages) = Channel::new();
        let _sink = stack.push(sink);
        emit(stack, events);
        assert_eq!(messages.try_recv(), Err(TryRecvError::Empty));
    }
}
