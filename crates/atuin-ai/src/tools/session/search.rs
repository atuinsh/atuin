//! `atuin_ai_session_search`: full-text search across captured AI-agent session transcripts.

use std::fmt::Write as _;

use atuin_client::ai_session::{HarnessKind, SessionMatch};
use atuin_client::settings::Settings;
use atuin_common::range::Clamped;
use atuin_common::string::NonBlankString;
use atuin_common::time::UtcOffsetExt;
use atuin_daemon::AiClient;
use futures::{StreamExt, TryStreamExt};
use schemars::JsonSchema;
use serde::Deserialize;

use super::caller::{Caller, is_own};
use super::{
    HarnessFilter, connect, cwd_filter, elsewhere_note, one_line, render_session_summary,
    resolve_cwd,
};
use crate::tools::ToolOutcome;

// Doc comments on the fields are the descriptions the model reads in the tool schema.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct AtuinAiSessionSearchToolCall {
    /// Words to look for across captured AI-agent session transcripts (what the user and agent
    /// wrote, the names of tools used, and session titles), case-insensitive. Sessions with a
    /// message containing every word rank first; if there are none, sessions matching any of the
    /// words (as prefixes) are returned instead, and the result says so. Use a few distinctive
    /// words, e.g. a file name, error text or identifier, not a sentence.
    pub query: NonBlankString,
    /// Maximum number of sessions to return, most relevant first.
    #[serde(default)]
    pub limit: Clamped<u32, 1, 20, 5>,
    /// Restrict the search to sessions from one AI harness. Omit to search every harness.
    #[serde(default)]
    pub harness: Option<HarnessFilter>,
    /// Only sessions whose working directory is this path or inside it. Relative paths,
    /// including '.', resolve against the current project directory. The result says how many
    /// matches the filter left out, and where.
    #[serde(default)]
    pub cwd: Option<String>,
}

impl AtuinAiSessionSearchToolCall {
    pub(crate) async fn execute(&self, settings: &Settings, caller: &Caller<'_>) -> ToolOutcome {
        let mut client = match connect(settings).await {
            Ok(client) => client,
            Err(outcome) => return outcome,
        };

        let harness = self.harness.map(HarnessKind::from);
        let cwd = match cwd_filter(self.cwd.as_deref()).map(resolve_cwd).transpose() {
            Ok(cwd) => cwd,
            Err(outcome) => return outcome,
        };
        let cwd_str = cwd.as_ref().map(|c| c.to_string_lossy());
        let own = caller.own_session_id(&mut client).await;
        let limit = self.limit.get();

        // Every term, as whole words, first; when no message anywhere has them all, any term as a
        // prefix, so a near-miss query still finds something instead of costing the model a
        // retry. Two things keep the looser search from answering a question nobody asked: what
        // counts is what matched before the caller's own session is set aside, and a strict match
        // outside the cwd filter (the same project checked out elsewhere) wins over loose matches
        // inside it; the note below then points at it.
        // One extra, so the page stays full after the caller's own session is dropped.
        let fetch = limit + 1;
        let strict =
            match search(&mut client, &self.query, harness, cwd_str.as_deref(), false, fetch).await
            {
                Ok(found) => visible(found, own.as_deref(), limit as usize),
                Err(e) => return ToolOutcome::Error(e),
            };
        let strict_elsewhere = match (&strict, cwd_str.as_deref()) {
            (Found::Nothing, Some(_)) => {
                match search(&mut client, &self.query, harness, None, false, fetch).await {
                    Ok(found) => Some(visible(found, own.as_deref(), limit as usize)),
                    Err(e) => return ToolOutcome::Error(e),
                }
            }
            _ => None,
        };
        let any_term = should_loosen(&strict, strict_elsewhere.as_ref());
        let (hits, only_own) = if any_term {
            match search(&mut client, &self.query, harness, cwd_str.as_deref(), true, fetch).await {
                Ok(found) => match visible(found, own.as_deref(), limit as usize) {
                    Found::Hits(hits) => (hits, false),
                    Found::OnlyOwn => (Vec::new(), true),
                    Found::Nothing => (Vec::new(), false),
                },
                Err(e) => return ToolOutcome::Error(e),
            }
        } else {
            match strict {
                Found::Hits(hits) => (hits, false),
                Found::OnlyOwn => (Vec::new(), true),
                Found::Nothing => (Vec::new(), false),
            }
        };

        // A cwd filter can hide the answer (the same project checked out elsewhere, or synced
        // from another machine), so say what it left out.
        let note = match cwd.as_deref() {
            Some(root) => search(&mut client, &self.query, harness, None, any_term, 20)
                .await
                .ok()
                .and_then(|all| {
                    let others = all
                        .iter()
                        .map(|hit| &hit.session)
                        .filter(|s| !is_own(s, own.as_deref()))
                        .collect::<Vec<_>>();
                    elsewhere_note(root, others, false)
                }),
            None => None,
        };

        if hits.is_empty() {
            let scope = cwd.map_or_else(String::new, |c| format!(" under {}", c.display()));
            let note = note.map_or_else(String::new, |n| format!(" {n}"));
            if only_own {
                return ToolOutcome::Success(format!(
                    "The only captured AI session{scope} matching {:?} is this one (the session \
                     calling this tool).{note}",
                    self.query.trim()
                ));
            }
            if !any_term {
                return ToolOutcome::Success(format!(
                    "No captured AI session{scope} has a message containing every word of \
                     {:?}.{note}",
                    self.query.trim()
                ));
            }
            return ToolOutcome::Success(format!(
                "No captured AI sessions{scope} matched any word of {:?}, even as a prefix.{note} \
                 Try other words, or atuin_ai_session_list to browse sessions by recency and \
                 directory.",
                self.query.trim()
            ));
        }

        let offset = time::UtcOffset::local_or_utc();
        let mut out = String::new();
        if any_term {
            let _ = writeln!(
                out,
                "No single message contained every word, so these sessions match some of them (as \
                 prefixes); check the match lines for relevance.\n"
            );
        }
        for (index, hit) in hits.iter().enumerate() {
            SessionHit(hit).render_into(&mut out, index + 1, offset);
        }
        if let Some(note) = note {
            let _ = writeln!(out, "\n{note}");
        }
        let _ = write!(
            out,
            "\nRead around a match with atuin_ai_session_read, e.g. start a few messages before \
             the matched message number."
        );
        ToolOutcome::Success(out)
    }
}

async fn search(
    client: &mut AiClient,
    query: &str,
    harness: Option<HarnessKind>,
    cwd: Option<&str>,
    any_term: bool,
    limit: u32,
) -> Result<Vec<SessionMatch>, String> {
    client
        .search_sessions(query, harness, cwd, any_term, limit)
        .await
        .map_err(|e| format!("AI session search failed: {e}"))?
        .map(|hit| {
            hit.map_err(|e| format!("AI session search failed: {e}")).and_then(|hit| {
                SessionMatch::try_from(hit)
                    .map_err(|e| format!("AI session search returned a malformed result: {e}"))
            })
        })
        .try_collect()
        .await
}

/// What a search found once the caller's own session is set aside.
#[derive(Debug)]
enum Found {
    Hits(Vec<SessionMatch>),
    /// Something matched, but only the caller's own session.
    OnlyOwn,
    Nothing,
}

/// Whether to fall back to any-term matching: only when the strict search matched nothing in the
/// requested scope and, when that scope is a directory, nothing outside it either.
fn should_loosen(in_scope: &Found, everywhere: Option<&Found>) -> bool {
    matches!(in_scope, Found::Nothing) && everywhere.is_none_or(|f| matches!(f, Found::Nothing))
}

fn visible(found: Vec<SessionMatch>, own: Option<&str>, limit: usize) -> Found {
    if found.is_empty() {
        return Found::Nothing;
    }
    let hits: Vec<_> =
        found.into_iter().filter(|hit| !is_own(&hit.session, own)).take(limit).collect();
    if hits.is_empty() {
        Found::OnlyOwn
    } else {
        Found::Hits(hits)
    }
}

struct SessionHit<'a>(&'a SessionMatch);

impl SessionHit<'_> {
    fn render_into(&self, out: &mut String, index: usize, offset: time::UtcOffset) {
        render_session_summary(out, index, &self.0.session, offset);
        let preview = one_line(&self.0.preview.to_plain().text, 400);
        if !preview.is_empty() {
            let _ = writeln!(out, "   match (message #{}): {preview}", self.0.message_index);
        }
    }
}

#[cfg(test)]
mod tests {
    use atuin_common::string::highlighted::TextHighlighter;
    use rstest::rstest;
    use serde_json::json;

    use super::*;

    #[rstest]
    #[case::default(json!({"query": "flaky test"}), 5)]
    #[case::clamped_high(json!({"query": "x", "limit": 100}), 20)]
    #[case::clamped_low(json!({"query": "x", "limit": 0}), 1)]
    #[case::null_limit(json!({"query": "x", "limit": null}), 5)]
    fn parses_query_and_clamps_limit(#[case] input: serde_json::Value, #[case] limit: u32) {
        let call: AtuinAiSessionSearchToolCall = serde_json::from_value(input).unwrap();
        assert_eq!(call.limit.get(), limit);
    }

    #[rstest]
    #[case::missing(json!({}))]
    #[case::empty(json!({"query": ""}))]
    #[case::blank(json!({"query": "   "}))]
    #[case::unknown_harness(json!({"query": "x", "harness": "emacs"}))]
    #[case::uncapturable_harness(json!({"query": "x", "harness": "copilot"}))]
    fn rejects_invalid_input(#[case] input: serde_json::Value) {
        assert!(serde_json::from_value::<AtuinAiSessionSearchToolCall>(input).is_err());
    }

    #[rstest]
    #[case::claude_code("claude-code", HarnessKind::ClaudeCode)]
    #[case::codex("codex", HarnessKind::Codex)]
    #[case::opencode("opencode", HarnessKind::Opencode)]
    #[case::pi("pi", HarnessKind::Pi)]
    fn parses_each_harness(#[case] name: &str, #[case] expected: HarnessKind) {
        let input = json!({"query": "x", "harness": name});
        let call: AtuinAiSessionSearchToolCall = serde_json::from_value(input).unwrap();
        assert_eq!(HarnessKind::from(call.harness.unwrap()), expected);
    }

    fn hit(id: &str) -> SessionMatch {
        SessionMatch {
            session: super::super::fixtures::session(id, None, time::OffsetDateTime::UNIX_EPOCH),
            title: TextHighlighter::default().as_highlighted(String::new()),
            preview: TextHighlighter::default().as_highlighted(String::new()),
            message_index: 0,
            score: 1.0,
        }
    }

    #[rstest]
    fn a_match_on_only_the_callers_own_session_is_not_a_miss() {
        // A strict miss falls back to a looser search; a strict hit on the caller's own session
        // must not, or the model is told no message had every word when one did.
        assert!(matches!(visible(vec![hit("me")], Some("me"), 5), Found::OnlyOwn));
        assert!(matches!(visible(vec![], Some("me"), 5), Found::Nothing));
        let Found::Hits(hits) = visible(vec![hit("me"), hit("a"), hit("b")], Some("me"), 1) else {
            panic!("other sessions matched");
        };
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].session.handle.session.as_ref(), "a");
    }

    #[rstest]
    fn a_strict_match_outside_the_directory_beats_loose_matches_inside_it() {
        let exact_elsewhere = Found::Hits(vec![hit("elsewhere")]);
        assert!(!should_loosen(&Found::Nothing, Some(&exact_elsewhere)));
        assert!(!should_loosen(&Found::Nothing, Some(&Found::OnlyOwn)));
        assert!(should_loosen(&Found::Nothing, Some(&Found::Nothing)));
        assert!(should_loosen(&Found::Nothing, None), "no cwd filter: nothing anywhere");
        assert!(!should_loosen(&Found::OnlyOwn, None));
        assert!(!should_loosen(&Found::Hits(vec![hit("a")]), None));
    }

    #[rstest]
    fn renders_a_hit_with_harness_time_title_directory_and_snippet() {
        let mut session = super::super::fixtures::session(
            "abc-123",
            Some("/work/atuin"),
            time::OffsetDateTime::UNIX_EPOCH,
        );
        session.title = Some("Add FTS".to_owned());
        session.last_reply = Some("Done: the index is\nbackfilled.".to_owned());
        let hit = SessionMatch {
            session,
            title: TextHighlighter::default().as_highlighted("Add FTS".to_owned()),
            preview: TextHighlighter::default().as_highlighted("the flaky test".to_owned()),
            message_index: 42,
            score: 1.0,
        };

        let mut out = String::new();
        SessionHit(&hit).render_into(&mut out, 1, time::UtcOffset::UTC);

        assert!(out.contains("claude-code"));
        assert!(out.contains("abc-123"));
        assert!(out.contains("Add FTS"));
        assert!(out.contains("the flaky test"));
        assert!(out.contains("in /work/atuin"));
        assert!(out.contains("message #42"));
        assert!(out.contains("last reply: Done: the index is backfilled."), "{out}");
    }
}
