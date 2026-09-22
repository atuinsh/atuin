use std::fmt::Write as _;

use atuin_client::settings::Settings;
use atuin_common::range::Clamped;
use atuin_common::string::NonBlankString;
use atuin_common::time::UtcOffsetExt;
use atuin_daemon::AiClient;
use atuin_daemon::grpc::ai_agent::pb::HarnessKind;
use atuin_daemon::grpc::ai_session::pb::SearchSessionsMatch;
use futures::TryStreamExt;
use schemars::JsonSchema;
use serde::Deserialize;

use crate::tools::ToolOutcome;

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct AtuinAiSessionSearchToolCall {
    pub query: NonBlankString,
    #[serde(default)]
    pub limit: Clamped<u32, 1, 20, 5>,
    #[serde(default)]
    pub harness: Option<HarnessFilter>,
}

#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum HarnessFilter {
    ClaudeCode,
    Codex,
    Copilot,
    Opencode,
    Pi,
}

impl From<HarnessFilter> for HarnessKind {
    fn from(value: HarnessFilter) -> Self {
        match value {
            HarnessFilter::ClaudeCode => Self::ClaudeCode,
            HarnessFilter::Codex => Self::Codex,
            HarnessFilter::Copilot => Self::Copilot,
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

struct SessionHit<'a>(&'a SearchSessionsMatch);

impl SessionHit<'_> {
    fn render_into(&self, out: &mut String, index: usize, offset: time::UtcOffset) {
        let session = self.0.session.as_ref();
        let id = session.map_or("", |s| s.session_id.as_str());
        let harness = session.map_or("unknown", |s| Self::harness_label(s.harness));
        let when = session
            .and_then(|s| s.updated_at.as_ref())
            .map_or_else(|| "unknown time".to_owned(), |ts| Self::timestamp(ts, offset));

        let _ = writeln!(out, "{index}. [{harness}] {when}  {id}");

        if let Some(title) = self.0.title.as_ref().map(|t| t.plain()) {
            let title = title.trim();
            if !title.is_empty() {
                let _ = writeln!(out, "   title: {title}");
            }
        }
        if let Some(preview) = self.0.preview.as_ref().map(|p| p.plain()) {
            let preview = preview.trim();
            if !preview.is_empty() {
                let _ = writeln!(out, "   match: {preview}");
            }
        }
    }

    fn harness_label(harness: i32) -> &'static str {
        match HarnessKind::try_from(harness) {
            Ok(HarnessKind::ClaudeCode) => "claude-code",
            Ok(HarnessKind::Codex) => "codex",
            Ok(HarnessKind::Copilot) => "copilot",
            Ok(HarnessKind::Opencode) => "opencode",
            Ok(HarnessKind::Pi) => "pi",
            Ok(HarnessKind::Unknown) | Err(_) => "unknown",
        }
    }

    fn timestamp(ts: &prost_types::Timestamp, offset: time::UtcOffset) -> String {
        match time::OffsetDateTime::from_unix_timestamp(ts.seconds) {
            Ok(when) => {
                let local = when.to_offset(offset);
                format!("{} {:02}:{:02}", local.date(), local.hour(), local.minute())
            }
            Err(_) => "unknown time".to_owned(),
        }
    }
}

#[cfg(test)]
mod tests {
    use atuin_common::string::highlighted::TextHighlighter;
    use atuin_daemon::grpc::ai_agent::pb::Session;
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
    fn rejects_invalid_input(#[case] input: serde_json::Value) {
        assert!(serde_json::from_value::<AtuinAiSessionSearchToolCall>(input).is_err());
    }

    #[rstest]
    #[case::claude_code("claude-code", HarnessKind::ClaudeCode)]
    #[case::codex("codex", HarnessKind::Codex)]
    #[case::copilot("copilot", HarnessKind::Copilot)]
    #[case::opencode("opencode", HarnessKind::Opencode)]
    #[case::pi("pi", HarnessKind::Pi)]
    fn parses_each_harness(#[case] name: &str, #[case] expected: HarnessKind) {
        let input = json!({"query": "x", "harness": name});
        let call: AtuinAiSessionSearchToolCall = serde_json::from_value(input).unwrap();
        assert_eq!(HarnessKind::from(call.harness.unwrap()), expected);
    }

    #[rstest]
    fn renders_a_hit_with_harness_time_title_and_snippet() {
        let hit = SearchSessionsMatch {
            session: Some(Session {
                harness: HarnessKind::ClaudeCode as i32,
                session_id: "abc-123".to_owned(),
                updated_at: Some(prost_types::Timestamp {
                    seconds: 0,
                    nanos: 0,
                }),
                ..Session::default()
            }),
            title: Some((&TextHighlighter::default().as_highlighted("Add FTS".to_owned())).into()),
            preview: Some(
                (&TextHighlighter::default().as_highlighted("the flaky test".to_owned())).into(),
            ),
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
