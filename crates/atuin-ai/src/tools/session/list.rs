//! `atuin_threads_list`: recent AI-agent sessions, newest first.

use std::fmt::Write as _;

use atuin_client::settings::{Dialect, ParseDateError, Settings};
use atuin_common::range::Clamped;
use jiff::ZonedDateTime;
use jiff::tz::TimeZone;
use schemars::JsonSchema;
use serde::Deserialize;

use super::caller::{Caller, is_own};
use super::{connect, elsewhere_note, is_subagent, render_session_summary, resolve_cwd};
use crate::commands::session::HarnessArg;
use crate::tools::ToolOutcome;

// Doc comments on the fields are the descriptions the model reads in the tool schema.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AtuinThreadsListToolCall {
    /// Only sessions whose working directory is this path or inside it. Relative paths,
    /// including '.', resolve against the current project directory, so '.' lists the sessions
    /// for this project.
    #[serde(default)]
    pub cwd: Option<String>,
    /// Only sessions from this AI harness. Omit for every harness.
    #[serde(default)]
    pub harness: Option<HarnessArg>,
    /// Only sessions active at or after this time, e.g. 'today', 'yesterday', '3 days ago',
    /// '2026-09-01'. Relative dates are in the user's local time.
    #[serde(default)]
    pub since: Option<String>,
    /// Maximum number of sessions to return, newest first.
    #[serde(default)]
    pub limit: Clamped<u32, 1, 50, 10>,
    /// Include subagent sessions (spawned by another agent session to do part of its work). Off
    /// by default: they are fragments of their parent and usually crowd out the sessions a person
    /// ran. Forks and continuations of a session are always listed, marked with where they came
    /// from.
    #[serde(default)]
    pub include_subagents: bool,
}

impl AtuinThreadsListToolCall {
    pub(crate) async fn execute(&self, settings: &Settings, caller: &Caller<'_>) -> ToolOutcome {
        let mut client = connect(settings).await?;
        let harness = self.harness.map(Into::into);
        let root = resolve_cwd(self.cwd.as_deref())?;
        let limit = self.limit.get() as usize;
        let tz = TimeZone::system();

        let since = match self.since.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            None => None,
            Some(since) => {
                let now = jiff::Timestamp::now().to_zoned_datetime(tz.clone());

                match parse_since(since, &now, settings.dialect) {
                    Ok(parsed) => Some(parsed),
                    Err(e) => {
                        return Err(format!(
                            "Could not parse since {since:?} ({e}). Try 'today', '2 days ago' or \
                             a date like '2026-09-01'."
                        ));
                    }
                }
            }
        };

        // The whole listing, not just a page: it also identifies the caller's own session, and
        // with a cwd filter, the sessions outside it say whether the project lives elsewhere too.
        // `since` is applied after identifying the caller: filtering first can drop the caller's
        // idle session and leave a parallel agent's as the lone candidate, hiding it. So fetch
        // back to whichever is earlier, the cutoff or what identifying the caller needs.
        let fetch_since = since.map(|since| since.min(caller.active_since()));
        let all = match super::list_sessions(&mut client, harness, fetch_since).await {
            Ok(all) => all,
            Err(e) => return Err(format!("Listing threads failed: {e}")),
        };
        let own = caller.own_in(&all);
        let (mut sessions, elsewhere): (Vec<_>, Vec<_>) = all
            .into_iter()
            .filter(|s| {
                !is_own(s, own.as_ref())
                    && (self.include_subagents || !is_subagent(s))
                    && since.is_none_or(|since| s.updated_at >= since)
            })
            .partition(|s| {
                root.as_deref()
                    .is_none_or(|root| s.cwd.as_deref().is_some_and(|c| c.starts_with(root)))
            });
        let note = root.as_deref().and_then(|root| elsewhere_note(root, &elsewhere, true, false));
        sessions.truncate(limit);

        if sessions.is_empty() {
            let mut scope = root.map_or_else(String::new, |r| format!(" under {}", r.display()));
            if let Some(since) = since {
                let _ = write!(scope, " active since {}", super::timestamp(since, &tz));
            }
            let note = note.map_or_else(String::new, |n| format!(" {n}"));
            return Ok(format!(
                "No captured threads{scope}.{note} Only threads recorded while thread capture was \
                 enabled are available."
            ));
        }

        let mut out = String::new();
        for (index, session) in sessions.iter().enumerate() {
            render_session_summary(&mut out, index + 1, session, &tz);
        }
        if let Some(note) = note {
            let _ = writeln!(out, "\n{note}");
        }
        let _ = write!(out, "\nRead one with atuin_threads_read and its thread id.");
        Ok(out)
    }
}

/// [`Dialect::parse_date`] resolves "today" and "yesterday" to now's time of day on that date,
/// but as a lower bound they mean the whole day. That day starts at its own midnight in `now`'s
/// zone, so a DST change since doesn't shift the bound by an hour.
fn parse_since(
    since: &str,
    now: &ZonedDateTime,
    dialect: Dialect,
) -> Result<jiff::Timestamp, ParseDateError> {
    let parsed = dialect.parse_date(since, now)?;

    if !since.eq_ignore_ascii_case("today") && !since.eq_ignore_ascii_case("yesterday") {
        return Ok(parsed.timestamp());
    }

    // The date the parse landed on at the offset it worked in, which is `now`'s.
    let date = now.offset().to_datetime(parsed.timestamp()).date();
    date.to_zoned_datetime(now.time_zone().clone())
        .map(|midnight| midnight.timestamp())
        .map_err(|_| ParseDateError::OutOfRange)
}

#[cfg(test)]
mod tests {
    use jiff::tz::offset;
    use rstest::rstest;
    use serde_json::json;

    use super::*;

    #[rstest]
    #[case("today", "2026-09-29T00:00:00+01:00".parse::<jiff::Timestamp>().unwrap())]
    #[case("Yesterday", "2026-09-28T00:00:00+01:00".parse::<jiff::Timestamp>().unwrap())]
    #[case("3 hours ago", "2026-09-29T16:30:00+01:00".parse::<jiff::Timestamp>().unwrap())]
    #[case("2026-09-01", "2026-09-01T00:00:00+01:00".parse::<jiff::Timestamp>().unwrap())]
    fn since_bounds(#[case] since: &str, #[case] expected: jiff::Timestamp) {
        let now = "2026-09-29T19:30:00+01:00"
            .parse::<jiff::Timestamp>()
            .unwrap()
            .to_zoned_datetime(TimeZone::fixed(offset(1)));

        assert_eq!(parse_since(since, &now, Dialect::Uk).unwrap(), expected);
    }

    // US Eastern falls back from -4 to -5 at 06:00 UTC on 2026-11-01.
    #[rstest]
    #[case("today", "2026-11-01T00:00:00-04:00".parse::<jiff::Timestamp>().unwrap())]
    #[case("yesterday", "2026-10-31T00:00:00-04:00".parse::<jiff::Timestamp>().unwrap())]
    fn since_bounds_across_dst(#[case] since: &str, #[case] expected: jiff::Timestamp) {
        let now = "2026-11-01T19:30:00-05:00"
            .parse::<jiff::Timestamp>()
            .unwrap()
            .to_zoned_datetime(TimeZone::get("America/New_York").unwrap());

        assert_eq!(parse_since(since, &now, Dialect::Uk).unwrap(), expected);
    }

    #[rstest]
    fn defaults() {
        let call: AtuinThreadsListToolCall = serde_json::from_value(json!({})).unwrap();
        assert_eq!(call.limit.get(), 10);
        assert!(!call.include_subagents);
        assert!(call.cwd.is_none());
        assert!(call.since.is_none());
    }
}
