//! The log file's line format.

use std::fmt;

use tracing::{Event, Subscriber};
use tracing_subscriber::fmt::format::Writer;
use tracing_subscriber::fmt::{FmtContext, FormatEvent, FormatFields};
use tracing_subscriber::registry::LookupSpan;

use super::Process;

/// Starts every line of an inner format with `process=… pid=… `.
///
/// A span entered at startup would not do: events from other threads (`spawn_blocking`, fjall's
/// own threads) have no current span.
pub(super) struct ProcessTagged<F> {
    prefix: String,
    inner: F,
}

impl<F> ProcessTagged<F> {
    pub(super) fn new(process: Process, inner: F) -> Self {
        Self {
            prefix: format!("process={process} pid={} ", std::process::id()),
            inner,
        }
    }
}

impl<S, N, F> FormatEvent<S, N> for ProcessTagged<F>
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
    F: FormatEvent<S, N>,
{
    fn format_event(
        &self,
        ctx: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &Event<'_>,
    ) -> fmt::Result {
        writer.write_str(&self.prefix)?;
        self.inner.format_event(ctx, writer, event)
    }
}
