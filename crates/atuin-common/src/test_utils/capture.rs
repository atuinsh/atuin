//! Utilities for capturing logs emitted by [`tracing`].
use std::sync::{Arc, Mutex, MutexGuard};

use tracing::field::{Field, Visit};
use tracing::subscriber::DefaultGuard;
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};

/// Start capturing logs from [`tracing`].
///
/// Logs will continue to be captured until the returned [`CapturedLogs`] object is dropped.
pub fn capture_logs() -> CapturedLogs {
    let logs: Arc<Mutex<Vec<LogItem>>> = Arc::default();
    let subscriber = tracing_subscriber::registry().with(CaptureLayer { logs: logs.clone() });
    let guard = tracing::subscriber::set_default(subscriber);
    CapturedLogs {
        logs,
        _guard: guard,
    }
}

/// An individual log item.
pub struct LogItem {
    pub level: Level,
    pub message: String,
}

/// Provides access to captured logs.
pub struct CapturedLogs {
    logs: Arc<Mutex<Vec<LogItem>>>,
    _guard: DefaultGuard,
}

impl CapturedLogs {
    /// Get the captured logs.
    pub fn get(&self) -> MutexGuard<'_, Vec<LogItem>> {
        self.logs.lock().unwrap()
    }
}

struct CaptureLayer {
    logs: Arc<Mutex<Vec<LogItem>>>,
}

impl<S: Subscriber> Layer<S> for CaptureLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        struct Visitor(Option<String>);

        impl Visit for Visitor {
            fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
                if field.name() == "message" {
                    // `value` is most likely of type `std::fmt::Arguments`, whose
                    // `Debug` impl is the same as `Display`.
                    self.0 = Some(format!("{value:?}"));
                }
            }
        }

        let mut visitor = Visitor(None);
        event.record(&mut visitor);
        if let Some(message) = visitor.0 {
            self.logs.lock().unwrap().push(LogItem {
                level: *event.metadata().level(),
                message,
            });
        }
    }
}
