//! `atuin_ai_session_list`: recent AI-agent sessions, newest first.

use std::fmt::Write as _;

use atuin_client::settings::Settings;
use atuin_common::range::Clamped;
use atuin_common::time::UtcOffsetExt;
use schemars::JsonSchema;
use serde::Deserialize;

use super::caller::{Caller, is_own};
use super::{connect, elsewhere_note, is_subagent, render_session_summary, resolve_cwd};
use crate::commands::session::HarnessArg;
use crate::tools::ToolOutcome;

// Doc comments on the fields are the descriptions the model reads in the tool schema.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct AtuinAiSessionListToolCall {
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

impl AtuinAiSessionListToolCall {
    pub(crate) async fn execute(&self, settings: &Settings, caller: &Caller<'_>) -> ToolOutcome {
        let mut client = match connect(settings).await {
            Ok(client) => client,
            Err(outcome) => return outcome,
        };
        let harness = self.harness.map(Into::into);
        let root = match resolve_cwd(self.cwd.as_deref()) {
            Ok(root) => root,
            Err(outcome) => return outcome,
        };
        let limit = self.limit.get() as usize;
        let offset = time::UtcOffset::local_or_utc();

        let since = match self.since.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            None => None,
            Some(since) => {
                let now = time::OffsetDateTime::now_utc().to_offset(offset);
                match interim::parse_date_string(since, now, settings.dialect.into()) {
                    Ok(parsed) => Some(parsed),
                    Err(e) => {
                        return ToolOutcome::Error(format!(
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
            Err(e) => return ToolOutcome::Error(format!("Listing AI sessions failed: {e}")),
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
                let _ = write!(scope, " active since {}", super::timestamp(since, offset));
            }
            let note = note.map_or_else(String::new, |n| format!(" {n}"));
            return ToolOutcome::Success(format!(
                "No captured AI sessions{scope}.{note} Only sessions recorded while AI session \
                 capture was enabled are available."
            ));
        }

        let mut out = String::new();
        for (index, session) in sessions.iter().enumerate() {
            render_session_summary(&mut out, index + 1, session, offset);
        }
        if let Some(note) = note {
            let _ = writeln!(out, "\n{note}");
        }
        let _ = write!(out, "\nRead one with atuin_ai_session_read and its session id.");
        ToolOutcome::Success(out)
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;
    use serde_json::json;

    use super::*;

    #[rstest]
    fn defaults() {
        let call: AtuinAiSessionListToolCall = serde_json::from_value(json!({})).unwrap();
        assert_eq!(call.limit.get(), 10);
        assert!(!call.include_subagents);
        assert!(call.cwd.is_none());
        assert!(call.since.is_none());
    }
}
