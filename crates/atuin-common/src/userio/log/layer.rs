//! The [`tracing`] layer that shows [`USER`] events on a [`SinkStack`].

use std::fmt;

use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::filter::{EnvFilter, Filtered, LevelFilter, Targets};
use tracing_subscriber::layer::{Context, Layer};
use tracing_subscriber::registry::LookupSpan;

use super::{Message, RequestId, Severity, SinkStack, USER, request};

/// Leaves [`USER`] events to the [`UserLayer`], for the filter of any other layer: a message shows
/// on the sink alone, not twice nor in logs and traces, which it would leak a login URL into.
pub trait UserFilterExt {
    /// This filter, dropping every [`USER`] event.
    #[must_use]
    fn without_user_messages(self) -> Self;
}

impl UserFilterExt for Targets {
    fn without_user_messages(self) -> Self {
        self.with_target(USER, LevelFilter::OFF)
    }
}

impl UserFilterExt for EnvFilter {
    fn without_user_messages(self) -> Self {
        self.add_directive(format!("{USER}=off").parse().expect("`USER` is a valid target"))
    }
}

/// Shows every [`USER`] event on the current sink of a [`SinkStack`], with the request of the
/// [`RequestId::span`] it was logged in.
pub struct UserLayer {
    stack: &'static SinkStack,
}

impl UserLayer {
    /// The [`tracing_subscriber::Layer`] for a `userio` [`SinkStack`], with a filter of its own.
    ///
    /// Add it last: a sink may log, and an event logged while another is dispatched clobbers the
    /// per-layer filter state of the layers still to see the first one.
    #[must_use]
    pub fn new<S>(stack: &'static SinkStack) -> Filtered<Self, Targets, S>
    where
        S: Subscriber + for<'lookup> LookupSpan<'lookup>,
    {
        Self { stack }.with_filter(Targets::new().with_target(USER, LevelFilter::INFO))
    }
}

impl<S> Layer<S> for UserLayer
where
    S: Subscriber + for<'lookup> LookupSpan<'lookup>,
{
    fn on_new_span(&self, attributes: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        let mut request = Request(None);
        attributes.record(&mut request);
        if let (Some(request), Some(span)) = (request.0, ctx.span(id)) {
            span.extensions_mut().insert(request);
        }
    }

    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        let Some(severity) = Severity::from_level(*event.metadata().level()) else {
            return;
        };

        let mut text = Text(None);
        event.record(&mut text);
        let Some(text) = text.0 else {
            return;
        };

        let request = ctx
            .event_scope(event)
            .and_then(|mut scope| scope.find_map(|span| span.extensions().get().copied()));

        self.stack.show(Message {
            severity,
            text,
            request,
        });
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

/// A [`RequestId::span`]'s request.
struct Request(Option<RequestId>);

impl Visit for Request {
    fn record_u128(&mut self, field: &Field, value: u128) {
        if field.name() == request::FIELD {
            self.0 = Some(RequestId::from_u128(value));
        }
    }

    fn record_debug(&mut self, _: &Field, _: &dyn fmt::Debug) {}
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
    use std::sync::Arc;

    use parking_lot::Mutex;
    use rstest::rstest;
    use tracing::{Event, Subscriber};
    use tracing_subscriber::Registry;
    use tracing_subscriber::filter::{EnvFilter, LevelFilter, Targets};
    use tracing_subscriber::layer::{Context, Filter, Layer, SubscriberExt as _};

    use super::{UserFilterExt, UserLayer};
    use crate::userio::log::{Held, RequestId, SinkStack, USER};

    /// Keeps the target of every event it sees.
    #[derive(Clone, Default)]
    struct Seen(Arc<Mutex<Vec<&'static str>>>);

    impl<S: Subscriber> Layer<S> for Seen {
        fn on_event(&self, event: &Event<'_>, _: Context<'_, S>) {
            self.0.lock().push(event.metadata().target());
        }
    }

    #[rstest]
    #[case::targets(Targets::new().with_default(LevelFilter::INFO))]
    #[case::env_filter(EnvFilter::new("info"))]
    // As `ATUIN_LOG` may: the filter's own directive for `USER` gives way.
    #[case::env_filter_naming_user(EnvFilter::new(format!("info,{USER}=info")))]
    fn a_filter_without_user_messages_drops_only_them(
        #[case] filter: impl UserFilterExt + Filter<Registry> + Send + Sync + 'static,
    ) {
        let seen = Seen::default();
        let subscriber = tracing_subscriber::registry()
            .with(seen.clone().with_filter(filter.without_user_messages()));

        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(target: USER, "for the user");
            tracing::info!(target: "atuin", "for the logs");
        });

        assert_eq!(*seen.0.lock(), ["atuin"]);
    }

    #[rstest]
    fn a_message_carries_the_request_it_was_shown_in() {
        static STACK: SinkStack = SinkStack::new();
        let shown = Held::default();
        let _shown = STACK.push(shown.clone());
        let request = RequestId::generate();
        // `Seen` keeps the spans other layers would see between the request and the message.
        let subscriber =
            tracing_subscriber::registry().with(Seen::default()).with(UserLayer::new(&STACK));

        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(target: USER, "outside");
            let _request = request.span().entered();
            let _work = tracing::info_span!("work").entered();
            tracing::info!(target: USER, "inside");
        });

        let shown: Vec<_> = shown.take().into_iter().map(|m| (m.text, m.request)).collect();
        assert_eq!(shown, [("outside".to_owned(), None), ("inside".to_owned(), Some(request))]);
    }
}
