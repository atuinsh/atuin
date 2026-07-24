use atuin_client::history::{History, is_known_agent};
use atuin_common::time::DATETIME_FMT;
use time::UtcOffset;

pub(crate) fn current_local_offset() -> UtcOffset {
    atuin_common::time::local_offset_or_utc()
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
        "[{}] (in `{}`, exit {}){}{}",
        format_timestamp(history, local_offset),
        history.cwd,
        history.exit,
        format_duration(history.duration),
        format_attribution(history)
    )
}

/// Attribution for agent-run commands: which agent, and its stated intent.
/// A stated intent marks a command as agent-run even when the agent is not in
/// KNOWN_AGENTS, so its author is still named. User-run commands (no intent,
/// author not a known agent) get nothing.
fn format_attribution(history: &History) -> String {
    match (is_known_agent(&history.author), &history.intent) {
        (true, Some(intent)) => format!(" — {}: {intent}", history.author),
        (true, None) => format!(" — {}", history.author),
        (false, Some(intent)) if !history.author.is_empty() => {
            format!(" — {}: {intent}", history.author)
        }
        (false, Some(intent)) => format!(" — intent: {intent}"),
        (false, None) => String::new(),
    }
}

fn format_timestamp(history: &History, local_offset: UtcOffset) -> String {
    history
        .timestamp
        .to_offset(local_offset)
        .format(DATETIME_FMT)
        .unwrap_or_else(|_| "????-??-?? ??:??:??".to_string())
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
            shell: Some("zsh".into()),
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

    #[test]
    fn formats_agent_attribution_with_intent() {
        let mut h = history(0);
        h.author = "claude-code".to_string();
        h.intent = Some("Run the test suite".to_string());

        assert_eq!(
            format_history_search_result(1, &h, UtcOffset::UTC),
            "## #1. (History ID: 018f011c-9a0a-7000-8000-000000000001):\n`cargo test`\n[1970-01-01 00:00:00] (in `/repo`, exit 2) — claude-code: Run the test suite\n"
        );
    }

    #[test]
    fn user_commands_have_no_attribution() {
        let mut h = history(0);
        h.author = "ellie".to_string();

        assert!(!format_history_search_result(1, &h, UtcOffset::UTC).contains('—'));
    }
}
