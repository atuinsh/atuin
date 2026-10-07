//! `atuin_ai_session_search`: full-text search across captured AI-agent session transcripts.

use std::fmt::Write as _;
use std::path::PathBuf;

use atuin_client::ai_session::{HarnessKind, SearchTerms, SessionFilter, SessionMatch};
use atuin_client::settings::Settings;
use atuin_common::range::Clamped;
use atuin_common::string::NonBlankString;
use atuin_common::time::UtcOffsetExt;
use atuin_daemon::AiClient;
use futures::{StreamExt, TryStreamExt};
use schemars::JsonSchema;
use serde::Deserialize;

use super::caller::{Caller, is_own};
use super::{connect, elsewhere_note, one_line, render_session_summary, resolve_cwd};
use crate::commands::session::HarnessArg;
use crate::tools::ToolOutcome;

/// How many sessions matching anywhere a cwd-filtered search scans to say what the filter left
/// out.
const NOTE_SCAN: u32 = 100;

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
    pub harness: Option<HarnessArg>,
    /// Only sessions whose working directory is this path or inside it. Relative paths,
    /// including '.', resolve against the current project directory. The result says how many
    /// matches the filter left out, and where.
    #[serde(default)]
    pub cwd: Option<String>,
}

impl AtuinAiSessionSearchToolCall {
    pub(crate) async fn execute(&self, settings: &Settings, caller: &Caller<'_>) -> ToolOutcome {
        self.run(settings, caller).await.map_or_else(|outcome| outcome, ToolOutcome::Success)
    }

    async fn run(&self, settings: &Settings, caller: &Caller<'_>) -> Result<String, ToolOutcome> {
        let mut client = connect(settings).await?;
        let harness = self.harness.map(HarnessKind::from);
        let cwd = resolve_cwd(self.cwd.as_deref())?;
        let cwd_str = cwd.as_ref().map(|c| c.to_string_lossy());
        let own = caller.own_session(&mut client).await;
        let limit = self.limit.get();
        // One extra, so the page stays full after the caller's own session is dropped.
        let fetch = limit + 1;
        let query = self.query.as_str();

        // Every term, as whole words, first; when no message anywhere has them all, any term as a
        // prefix, so a near-miss query still finds something instead of costing the model a
        // retry. See `should_loosen` for what keeps the looser search from answering a question
        // nobody asked.
        let strict = search(&mut client, query, harness, cwd_str.as_deref(), false, fetch).await?;
        // With a cwd filter, one unscoped strict search serves both that decision and the note.
        let unscoped = match cwd_str {
            Some(_) => {
                Some(search(&mut client, query, harness, None, false, fetch.max(NOTE_SCAN)).await?)
            }
            None => None,
        };
        let any_term = should_loosen(&strict, unscoped.as_deref());
        let found = if any_term {
            search(&mut client, query, harness, cwd_str.as_deref(), true, fetch).await?
        } else {
            strict
        };
        let hits: Vec<&SessionMatch> = found
            .iter()
            .filter(|hit| !is_own(&hit.session, own.as_ref()))
            .take(limit as usize)
            .collect();
        let only_own = hits.is_empty() && !found.is_empty();

        // A cwd filter can hide the answer (the same project checked out elsewhere, or synced
        // from another machine), so say what it left out.
        let note = match cwd.as_deref() {
            Some(root) => {
                let all = if any_term {
                    search(&mut client, query, harness, None, true, NOTE_SCAN).await.ok()
                } else {
                    unscoped
                };
                all.and_then(|all| {
                    // The top hits overall, so in-directory ones take some slots and a common
                    // query can match more sessions than are scanned: a full scan says so.
                    let partial = all.len() >= NOTE_SCAN as usize;
                    let others = all
                        .iter()
                        .map(|hit| &hit.session)
                        .filter(|s| !is_own(s, own.as_ref()))
                        .collect::<Vec<_>>();
                    elsewhere_note(root, others, false, partial)
                })
            }
            None => None,
        };

        if hits.is_empty() {
            let scope = cwd.map_or_else(String::new, |c| format!(" under {}", c.display()));
            let note = note.map_or_else(String::new, |n| format!(" {n}"));
            let query = self.query.trim();
            return Ok(if only_own {
                format!(
                    "The only captured AI session{scope} matching {query:?} is this one (the \
                     session calling this tool).{note}"
                )
            } else if !any_term {
                format!(
                    "No captured AI session{scope} has a message containing every word of \
                     {query:?}.{note}"
                )
            } else {
                format!(
                    "No captured AI sessions{scope} matched any word of {query:?}, even as a \
                     prefix.{note} Try other words, or atuin_ai_session_list to browse sessions \
                     by recency and directory."
                )
            });
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
             the matched message number. Each session shows only its best match; to find every \
             mention in one, read it with query."
        );
        Ok(out)
    }
}

async fn search(
    client: &mut AiClient,
    query: &str,
    harness: Option<HarnessKind>,
    cwd: Option<&str>,
    any_term: bool,
    limit: u32,
) -> Result<Vec<SessionMatch>, ToolOutcome> {
    let filter = SessionFilter {
        harness,
        workspace: cwd.map(PathBuf::from),
        ..SessionFilter::default()
    };
    let terms = if any_term {
        SearchTerms::Any
    } else {
        SearchTerms::All
    };
    client
        .search_sessions(query, terms, &filter, limit)
        .await
        .map_err(|e| ToolOutcome::Error(format!("AI session search failed: {e}")))?
        .map(|hit| {
            hit.map_err(|e| format!("AI session search failed: {e}")).and_then(|hit| {
                SessionMatch::try_from(hit)
                    .map_err(|e| format!("AI session search returned a malformed result: {e}"))
            })
        })
        .try_collect()
        .await
        .map_err(ToolOutcome::Error)
}

/// Whether to fall back to any-term matching: only when no message has every word, neither in
/// the requested scope nor, when that scope is a directory, outside it. Counted before the
/// caller's own session is set aside (its match is still a match), and a strict match elsewhere
/// wins over loose ones in the directory; the note then points at it.
fn should_loosen(strict: &[SessionMatch], unscoped: Option<&[SessionMatch]>) -> bool {
    strict.is_empty() && unscoped.is_none_or(<[SessionMatch]>::is_empty)
}

struct SessionHit<'a>(&'a SessionMatch);

impl SessionHit<'_> {
    fn render_into(&self, out: &mut String, index: usize, offset: time::UtcOffset) {
        render_session_summary(out, index, &self.0.session, offset);
        let preview = one_line(&self.0.preview.to_plain().text, 400);
        if !preview.is_empty() {
            // The message number is in the session holding it, which a grouped search can
            // return under its root: name that session, to read around the match in.
            let place = match &self.0.matched {
                Some(matched) => format!(" in {}", matched.handle.session),
                None => String::new(),
            };
            let _ = writeln!(out, "   match (message #{}{place}): {preview}", self.0.message_index);
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
            matched: None,
            score: 1.0,
        }
    }

    #[rstest]
    fn loosens_only_when_no_message_anywhere_has_every_word() {
        let some = [hit("a")];
        assert!(should_loosen(&[], None));
        assert!(should_loosen(&[], Some(&[])));
        assert!(!should_loosen(&some, None), "a strict hit, even the caller's own, is a hit");
        assert!(!should_loosen(&[], Some(&some)), "a strict hit elsewhere wins over loose ones");
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
            matched: None,
            score: 1.0,
        };

        let mut out = String::new();
        SessionHit(&hit).render_into(&mut out, 1, time::UtcOffset::UTC);

        assert!(out.contains("claude-code"));
        assert!(out.contains(&hit.session.atuin_id.to_string()));
        assert!(out.contains("Add FTS"));
        assert!(out.contains("the flaky test"));
        assert!(out.contains("in /work/atuin"));
        assert!(out.contains("message #42):"), "{out}");
        assert!(out.contains("last reply: Done: the index is backfilled."), "{out}");

        // A match in another session of the group names it.
        let child = SessionMatch {
            matched: Some(atuin_client::ai_session::MatchedSession {
                handle: atuin_client::ai_session::HarnessSession {
                    harness: HarnessKind::ClaudeCode,
                    session: "child-9".to_owned().into(),
                },
                title: TextHighlighter::default().as_highlighted(String::new()),
            }),
            ..hit
        };
        let mut out = String::new();
        SessionHit(&child).render_into(&mut out, 1, time::UtcOffset::UTC);
        assert!(out.contains("message #42 in child-9):"), "{out}");
    }
}
