use atuin_client::history::History;
use time::UtcOffset;

pub(crate) fn current_local_offset() -> UtcOffset {
    UtcOffset::current_local_offset().unwrap_or(UtcOffset::UTC)
}

pub(crate) fn format_last_command(history: &History, local_offset: UtcOffset) -> String {
    format!(
        "History ID: {} - `{}`\n{}",
        history.id,
        history.command,
        format_history_metadata(history, local_offset)
    )
}

pub(crate) fn format_history_search_result(
    ordinal: usize,
    history: &History,
    local_offset: UtcOffset,
) -> String {
    format!(
        "## #{}. (History ID: {}):\n`{}`\n{}\n",
        ordinal,
        history.id,
        history.command,
        format_history_metadata(history, local_offset)
    )
}

fn format_history_metadata(history: &History, local_offset: UtcOffset) -> String {
    format!(
        "[{}] (in `{}`, exit {}){}",
        format_timestamp(history, local_offset),
        history.cwd,
        history.exit,
        format_duration(history.duration)
    )
}

fn format_timestamp(history: &History, local_offset: UtcOffset) -> String {
    let ts = history.timestamp.to_offset(local_offset);
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        ts.year(),
        ts.month() as u8,
        ts.day(),
        ts.hour(),
        ts.minute(),
        ts.second(),
    )
}

fn format_duration(nanos: i64) -> String {
    if nanos <= 0 {
        return String::new();
    }

    let total_secs = nanos / 1_000_000_000;
    let millis = (nanos % 1_000_000_000) / 1_000_000;

    if total_secs >= 3600 {
        let hours = total_secs / 3600;
        let mins = (total_secs % 3600) / 60;
        let secs = total_secs % 60;
        format!(", {hours}h{mins}m{secs}s")
    } else if total_secs >= 60 {
        let mins = total_secs / 60;
        let secs = total_secs % 60;
        format!(", {mins}m{secs}s")
    } else if total_secs > 0 {
        if millis > 0 {
            format!(", {total_secs}.{millis:03}s")
        } else {
            format!(", {total_secs}s")
        }
    } else {
        format!(", {millis}ms")
    }
}

#[cfg(test)]
mod tests {
    use atuin_client::history::{History, HistoryId};
    use time::{OffsetDateTime, UtcOffset};

    use super::*;

    fn history(duration: i64) -> History {
        History {
            id: HistoryId("018f011c-9a0a-7000-8000-000000000001".to_string()),
            timestamp: OffsetDateTime::UNIX_EPOCH,
            duration,
            exit: 2,
            command: "cargo test".to_string(),
            cwd: "/repo".to_string(),
            session: String::new(),
            hostname: String::new(),
            author: String::new(),
            intent: None,
            deleted_at: None,
        }
    }

    #[test]
    fn formats_last_command() {
        assert_eq!(
            format_last_command(&history(1_234_000_000), UtcOffset::UTC),
            "History ID: 018f011c-9a0a-7000-8000-000000000001 - `cargo test`\n[1970-01-01 00:00:00] (in `/repo`, exit 2), 1.234s"
        );
    }

    #[test]
    fn formats_history_search_result() {
        assert_eq!(
            format_history_search_result(3, &history(0), UtcOffset::UTC),
            "## #3. (History ID: 018f011c-9a0a-7000-8000-000000000001):\n`cargo test`\n[1970-01-01 00:00:00] (in `/repo`, exit 2)\n"
        );
    }
}
