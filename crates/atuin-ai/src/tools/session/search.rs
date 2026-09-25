use std::fmt::Write as _;

use atuin_client::ai_session::{HarnessKind, SessionMatch};
use atuin_client::settings::Settings;
use atuin_common::range::Clamped;
use atuin_common::string::NonBlankString;
use atuin_common::time::UtcOffsetExt;
use atuin_daemon::AiClient;
use futures::TryStreamExt;
use serde::Deserialize;
use time::OffsetDateTime;

use crate::commands::session::harness_name;
use crate::tools::ToolOutcome;

#[derive(Debug, Clone, Deserialize)]
pub struct AtuinAiSessionSearchToolCall {
    pub query: NonBlankString,
    #[serde(default)]
    pub limit: Clamped<u32, 1, 20, 5>,
    #[serde(default)]
    pub harness: Option<HarnessFilter>,
}

// Only harnesses a capture path can actually produce are offered as filters (see AnyHarness);
// Copilot has no capture source yet, so advertising it would return empty for every query.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HarnessFilter {
    ClaudeCode,
    Codex,
    Opencode,
    Pi,
}

impl From<HarnessFilter> for HarnessKind {
    fn from(value: HarnessFilter) -> Self {
        match value {
            HarnessFilter::ClaudeCode => Self::ClaudeCode,
            HarnessFilter::Codex => Self::Codex,
            HarnessFilter::Opencode => Self::Opencode,
            HarnessFilter::Pi => Self::Pi,
        }
    }
}

impl AtuinAiSessionSearchToolCall {
    pub(crate) async fn execute(&self, settings: &Settings) -> ToolOutcome {
        let mut client = match AiClient::from_settings(settings).await {
            Ok(client) => client,
            Err(e) => {
                return ToolOutcome::Error(format!(
                    "AI session search is unavailable: could not connect to the Atuin daemon \
                     ({e}). Shell history search still works."
                ));
            }
        };

        let harness = self.harness.map(HarnessKind::from);
        let hits = async {
            client
                .search_sessions(self.query.as_str(), harness, self.limit.get())
                .await
                .map_err(|e| format!("AI session search failed: {e}"))?
                .map_err(|e| format!("AI session search failed: {e}"))
                .try_collect::<Vec<_>>()
                .await
        }
        .await;
        let hits = match hits {
            Ok(hits) => hits,
            Err(e) => return ToolOutcome::Error(e),
        };
        let hits = match hits.into_iter().map(SessionMatch::try_from).collect::<Result<Vec<_>, _>>()
        {
            Ok(hits) => hits,
            Err(e) => {
                return ToolOutcome::Error(format!(
                    "AI session search returned a malformed result: {e}"
                ));
            }
        };

        if hits.is_empty() {
            return ToolOutcome::Success(format!(
                "No captured AI sessions matched query {:?}. Only sessions recorded while AI \
                 session capture was enabled are searchable, and terms are AND-ed as whole words, \
                 so try fewer or different terms.",
                self.query.trim()
            ));
        }

        let offset = time::UtcOffset::local_or_utc();
        let mut out = String::new();
        for (index, hit) in hits.iter().enumerate() {
            SessionHit(hit).render_into(&mut out, index + 1, offset);
        }
        ToolOutcome::Success(out)
    }
}

struct SessionHit<'a>(&'a SessionMatch);

impl SessionHit<'_> {
    fn render_into(&self, out: &mut String, index: usize, offset: time::UtcOffset) {
        let session = &self.0.session;
        let id = &session.handle.session;
        let harness = harness_name(session.handle.harness);
        let when = Self::timestamp(session.updated_at, offset);

        let _ = writeln!(out, "{index}. [{harness}] {when}  {id}");

        let title = self.0.title.to_plain().text;
        let title = title.trim();
        if !title.is_empty() {
            let _ = writeln!(out, "   title: {title}");
        }
        let preview = self.0.preview.to_plain().text;
        let preview = preview.trim();
        if !preview.is_empty() {
            let _ = writeln!(out, "   match: {preview}");
        }
    }

    fn timestamp(ts: OffsetDateTime, offset: time::UtcOffset) -> String {
        match ts.checked_to_offset(offset) {
            Some(local) => format!("{} {:02}:{:02}", local.date(), local.hour(), local.minute()),
            None => "unknown time".to_owned(),
        }
    }
}

#[cfg(test)]
mod tests {
    use atuin_client::ai_session::{HarnessSession, NativeSessionId, Session};
    use atuin_common::harnesstools::session::Usage;
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

    #[rstest]
    fn renders_a_hit_with_harness_time_title_and_snippet() {
        let hit = SessionMatch {
            session: Session::builder()
                .handle(HarnessSession {
                    harness: HarnessKind::ClaudeCode,
                    session: NativeSessionId::from("abc-123".to_owned()),
                })
                .started_at(OffsetDateTime::UNIX_EPOCH)
                .updated_at(OffsetDateTime::UNIX_EPOCH)
                .usage(Usage::default())
                .build(),
            title: TextHighlighter::default().as_highlighted("Add FTS".to_owned()),
            preview: TextHighlighter::default().as_highlighted("the flaky test".to_owned()),
            score: 1.0,
        };

        let mut out = String::new();
        SessionHit(&hit).render_into(&mut out, 1, time::UtcOffset::UTC);

        assert!(out.contains("claude-code"));
        assert!(out.contains("abc-123"));
        assert!(out.contains("Add FTS"));
        assert!(out.contains("the flaky test"));
    }
}
