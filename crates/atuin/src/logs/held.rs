use std::fmt::{self, Write as _};
use std::sync::mpsc;

use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::layer::{Context, Layer, SubscriberExt as _};

/// The warnings and errors logged before logging is set up, as by loading the settings it is set up
/// from, held to log once it is.
pub struct HeldEvents(mpsc::Receiver<(Level, String)>);

impl HeldEvents {
    /// Runs `f`, holding the warnings and errors it logs.
    pub fn hold<T>(f: impl FnOnce() -> T) -> (T, Self) {
        let (sender, receiver) = mpsc::channel();
        let subscriber =
            tracing_subscriber::registry().with(Holder(sender).with_filter(LevelFilter::WARN));
        (tracing::subscriber::with_default(subscriber, f), Self(receiver))
    }

    /// Logs the held events through the logging set up since, if any.
    pub fn emit(self) {
        for (level, message) in self.0.try_iter() {
            if level == Level::ERROR {
                tracing::error!("{message}");
            } else {
                tracing::warn!("{message}");
            }
        }
    }
}

struct Holder(mpsc::Sender<(Level, String)>);

impl<S: Subscriber> Layer<S> for Holder {
    fn on_event(&self, event: &Event<'_>, _: Context<'_, S>) {
        let mut message = Message::default();
        event.record(&mut message);
        // The receiver outlives `hold`, the only place this layer runs.
        let _ = self.0.send((*event.metadata().level(), message.text + &message.fields));
    }
}

/// An event's message, then its other fields, as `tracing_subscriber::fmt` writes them.
#[derive(Default)]
struct Message {
    text: String,
    fields: String,
}

impl Visit for Message {
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        let _ = match field.name() {
            "message" => write!(self.text, "{value:?}"),
            name => write!(self.fields, " {name}={value:?}"),
        };
    }
}
