//! Line formats for the log file and for user messages.

use std::fmt;

use colored::{Color, Colorize};
use tracing::{Event, Level, Subscriber};
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

/// Formats user messages the way a CLI reports them: `warning: …`, `error: …`, and plain text
/// otherwise.
pub(super) struct UserFormat;

impl<S, N> FormatEvent<S, N> for UserFormat
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        ctx: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &Event<'_>,
    ) -> fmt::Result {
        let label = match *event.metadata().level() {
            Level::ERROR => Some(("error:", Color::Red)),
            Level::WARN => Some(("warning:", Color::Yellow)),
            _ => None,
        };
        if let Some((label, color)) = label {
            if writer.has_ansi_escapes() {
                write!(writer, "{} ", label.color(color).bold())?;
            } else {
                write!(writer, "{label} ")?;
            }
        }
        ctx.field_format().format_fields(writer.by_ref(), event)?;
        writeln!(writer)
    }
}

#[cfg(test)]
mod tests {
    use atuin_common::logs::USER;
    use rstest::{fixture, rstest};
    use tracing::Level;
    use tracing::dispatcher::{self, Dispatch};
    use tracing_subscriber::fmt;
    use tracing_subscriber::prelude::*;

    use super::UserFormat;
    use crate::logs::tests::Capture;

    struct Rendered {
        out: Capture,
        dispatch: Dispatch,
    }

    #[fixture]
    fn rendered() -> Rendered {
        let out = Capture::default();
        let layer = fmt::layer().event_format(UserFormat).with_writer(out.clone()).with_ansi(false);
        Rendered {
            dispatch: Dispatch::new(tracing_subscriber::registry().with(layer)),
            out,
        }
    }

    #[rstest]
    #[case::error(Level::ERROR, "error: boom\n")]
    #[case::warn(Level::WARN, "warning: boom\n")]
    #[case::info(Level::INFO, "boom\n")]
    fn user_format_prefixes_by_level(
        rendered: Rendered,
        #[case] level: Level,
        #[case] expected: &str,
    ) {
        dispatcher::with_default(&rendered.dispatch, || match level {
            Level::ERROR => tracing::error!(target: USER, "boom"),
            Level::WARN => tracing::warn!(target: USER, "boom"),
            _ => tracing::info!(target: USER, "boom"),
        });

        assert_eq!(rendered.out.text(), expected);
    }

    #[rstest]
    fn user_format_appends_fields(rendered: Rendered) {
        dispatcher::with_default(&rendered.dispatch, || {
            tracing::info!(target: USER, count = 3, "boom");
        });

        assert_eq!(rendered.out.text(), "boom count=3\n");
    }
}
