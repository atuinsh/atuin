//! Context window management for API requests.
//!
//! Full conversation events are always persisted to disk. This module handles
//! truncation at send time so the API payload stays within a character budget.
//!
//! Strategy: **frozen prefix + live tail**. The first N turns form a stable
//! prefix that stays identical across requests (maximizing prompt cache hits).
//! The most recent turns form the live tail. When the total exceeds the budget,
//! turns between prefix and tail are dropped with a truncation marker. The
//! prefix never shifts, avoiding cache invalidation.

use std::ops::Range;

use crate::tui::{ConversationEvent, events_to_messages};

/// Default character budget for the context window.
/// Roughly ~50K tokens at ~4 chars/token — generous enough that truncation
/// only kicks in for genuinely long sessions.
const DEFAULT_BUDGET_CHARS: usize = 200_000;

/// Number of initial turns to freeze as the stable prefix.
const FROZEN_PREFIX_TURNS: usize = 1;

/// Builds API messages from conversation events while respecting a character
/// budget using frozen prefix + live tail truncation.
pub(crate) struct ContextWindowBuilder {
    budget: usize,
}

impl ContextWindowBuilder {
    pub fn new(budget: usize) -> Self {
        Self { budget }
    }

    pub fn with_default_budget() -> Self {
        Self::new(DEFAULT_BUDGET_CHARS)
    }

    /// Build API messages from conversation events, applying the context
    /// window budget. Returns the messages to send in the API request.
    pub fn build(&self, events: &[ConversationEvent]) -> Vec<serde_json::Value> {
        if events.is_empty() {
            return Vec::new();
        }

        let turns = group_into_turns(events);

        // Convert each turn's events to API messages independently.
        // This is safe because the combining logic (Text + ToolCall merging)
        // only operates within a single assistant response, which never
        // spans turn boundaries.
        let turn_messages: Vec<Vec<serde_json::Value>> = turns
            .iter()
            .map(|range| events_to_messages(&events[range.clone()]))
            .collect();

        let turn_chars: Vec<usize> = turn_messages.iter().map(|m| estimate_chars(m)).collect();
        let total_chars: usize = turn_chars.iter().sum();

        if total_chars <= self.budget {
            return turn_messages.into_iter().flatten().collect();
        }

        // --- Over budget: apply frozen prefix + live tail ---

        let prefix_count = FROZEN_PREFIX_TURNS.min(turns.len());
        let prefix_chars: usize = turn_chars[..prefix_count].iter().sum();

        let marker = truncation_marker();
        let marker_chars = estimate_chars(std::slice::from_ref(&marker));

        let mut remaining = self.budget.saturating_sub(prefix_chars + marker_chars);

        // Work backwards from the end, accumulating tail turns that fit.
        let mut tail_start = turns.len();
        for i in (prefix_count..turns.len()).rev() {
            if turn_chars[i] <= remaining {
                remaining -= turn_chars[i];
                tail_start = i;
            } else {
                break;
            }
        }

        // Always include at least the most recent turn, even if it alone
        // exceeds the budget — sending something is better than nothing.
        if tail_start >= turns.len() && turns.len() > prefix_count {
            tail_start = turns.len() - 1;
        }

        let mut result = Vec::new();

        // Frozen prefix
        for msgs in &turn_messages[..prefix_count] {
            result.extend(msgs.iter().cloned());
        }

        // Truncation marker (only if turns were actually dropped)
        if tail_start > prefix_count {
            result.push(marker);
        }

        // Live tail
        for msgs in &turn_messages[tail_start..] {
            result.extend(msgs.iter().cloned());
        }

        result
    }
}

/// Marker message inserted where turns were dropped. Uses user role since
/// the preceding prefix typically ends with an assistant message.
fn truncation_marker() -> serde_json::Value {
    serde_json::json!({
        "role": "user",
        "content": "[Earlier conversation context was omitted to fit within the context window. The conversation continues below.]"
    })
}

/// Group conversation events into turns. A new turn starts at each
/// `UserMessage` or `SystemContext` event. Everything between boundaries
/// belongs to the preceding turn (assistant text, tool calls, tool results,
/// out-of-band output).
fn group_into_turns(events: &[ConversationEvent]) -> Vec<Range<usize>> {
    let mut turns = Vec::new();
    let mut start = 0;

    for (i, event) in events.iter().enumerate() {
        if i > start
            && matches!(
                event,
                ConversationEvent::UserMessage { .. } | ConversationEvent::SystemContext { .. }
            )
        {
            turns.push(start..i);
            start = i;
        }
    }

    if start < events.len() {
        turns.push(start..events.len());
    }

    turns
}

/// Rough character-count estimate for a set of messages. Uses the JSON
/// serialization length as a proxy — not exact tokens, but proportional
/// and cheap to compute.
fn estimate_chars(messages: &[serde_json::Value]) -> usize {
    messages.iter().map(|m| m.to_string().len()).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(content: &str) -> ConversationEvent {
        ConversationEvent::UserMessage {
            content: content.to_string(),
        }
    }

    fn text(content: &str) -> ConversationEvent {
        ConversationEvent::Text {
            content: content.to_string(),
        }
    }

    fn tool_call(id: &str, name: &str) -> ConversationEvent {
        ConversationEvent::ToolCall {
            id: id.to_string(),
            name: name.to_string(),
            input: serde_json::json!({"command": "ls"}),
        }
    }

    fn tool_result(tool_use_id: &str, content: &str) -> ConversationEvent {
        ConversationEvent::ToolResult {
            tool_use_id: tool_use_id.to_string(),
            content: content.to_string(),
            is_error: false,
            remote: false,
            content_length: None,
        }
    }

    fn system_context(content: &str) -> ConversationEvent {
        ConversationEvent::SystemContext {
            content: content.to_string(),
        }
    }

    fn oob(content: &str) -> ConversationEvent {
        ConversationEvent::OutOfBandOutput {
            name: "test".to_string(),
            command: None,
            content: content.to_string(),
        }
    }

    // --- group_into_turns ---

    #[test]
    fn empty_events_produce_no_turns() {
        assert!(group_into_turns(&[]).is_empty());
    }

    #[test]
    fn single_user_message_is_one_turn() {
        let events = vec![user("hello")];
        let turns = group_into_turns(&events);
        assert_eq!(turns, vec![0..1]);
    }

    #[test]
    fn user_assistant_is_one_turn() {
        let events = vec![user("hello"), text("hi there")];
        let turns = group_into_turns(&events);
        assert_eq!(turns, vec![0..2]);
    }

    #[test]
    fn two_turns_split_at_user_message() {
        let events = vec![
            user("first"),
            text("response 1"),
            user("second"),
            text("response 2"),
        ];
        let turns = group_into_turns(&events);
        assert_eq!(turns, vec![0..2, 2..4]);
    }

    #[test]
    fn tool_calls_and_results_stay_in_same_turn() {
        let events = vec![
            user("list files"),
            text("Let me check"),
            tool_call("tc1", "suggest_command"),
            tool_result("tc1", "file1\nfile2"),
            text("Here are your files"),
        ];
        let turns = group_into_turns(&events);
        assert_eq!(turns, vec![0..5]);
    }

    #[test]
    fn system_context_starts_new_turn() {
        let events = vec![
            user("hello"),
            text("hi"),
            system_context("invocation boundary"),
            user("next question"),
            text("answer"),
        ];
        let turns = group_into_turns(&events);
        assert_eq!(turns, vec![0..2, 2..3, 3..5]);
    }

    #[test]
    fn oob_events_stay_in_current_turn() {
        let events = vec![user("hello"), oob("some output"), text("response")];
        let turns = group_into_turns(&events);
        assert_eq!(turns, vec![0..3]);
    }

    #[test]
    fn leading_text_without_user_message() {
        // Edge case: events start with assistant text (shouldn't happen
        // normally but handle gracefully)
        let events = vec![text("orphaned"), user("hello"), text("hi")];
        let turns = group_into_turns(&events);
        assert_eq!(turns, vec![0..1, 1..3]);
    }

    // --- ContextWindowBuilder ---

    #[test]
    fn empty_events_produce_empty_messages() {
        let builder = ContextWindowBuilder::with_default_budget();
        assert!(builder.build(&[]).is_empty());
    }

    #[test]
    fn under_budget_returns_all_messages() {
        let events = vec![user("hello"), text("hi"), user("how are you"), text("good")];
        let builder = ContextWindowBuilder::with_default_budget();
        let messages = builder.build(&events);

        // Should produce 4 messages (2 user + 2 assistant)
        assert_eq!(messages.len(), 4);
        assert_eq!(messages[0]["role"], "user");
        assert_eq!(messages[0]["content"], "hello");
        assert_eq!(messages[1]["role"], "assistant");
        assert_eq!(messages[1]["content"], "hi");
        assert_eq!(messages[2]["role"], "user");
        assert_eq!(messages[2]["content"], "how are you");
        assert_eq!(messages[3]["role"], "assistant");
        assert_eq!(messages[3]["content"], "good");
    }

    #[test]
    fn over_budget_truncates_middle_turns() {
        // Create events where each turn has known content. Use a tiny
        // budget so truncation is triggered with just a few turns.
        let events = vec![
            user("turn-1-user"),
            text("turn-1-assistant"),
            user("turn-2-user"),
            text("turn-2-assistant"),
            user("turn-3-user"),
            text("turn-3-assistant"),
            user("turn-4-user"),
            text("turn-4-assistant-final"),
        ];

        // Calculate sizes to set budget that keeps turn 1 (prefix) + turn 4 (tail)
        // but drops turns 2 and 3.
        let all_messages = events_to_messages(&events);
        let total_chars: usize = all_messages.iter().map(|m| m.to_string().len()).sum();

        // Set budget to roughly half — enough for prefix + last turn + marker
        let turn1_msgs = events_to_messages(&events[0..2]);
        let turn4_msgs = events_to_messages(&events[6..8]);
        let marker_chars = estimate_chars(std::slice::from_ref(&truncation_marker()));
        let needed = estimate_chars(&turn1_msgs) + estimate_chars(&turn4_msgs) + marker_chars;

        // Budget allows prefix + marker + last turn but not the middle turns
        assert!(
            needed < total_chars,
            "test setup: needed ({needed}) should be less than total ({total_chars})"
        );
        let builder = ContextWindowBuilder::new(needed + 10); // small margin

        let messages = builder.build(&events);

        // Should have: turn 1 (2 msgs) + marker (1 msg) + turn 4 (2 msgs) = 5
        assert_eq!(messages.len(), 5, "expected prefix + marker + tail");
        assert_eq!(messages[0]["content"], "turn-1-user");
        assert_eq!(messages[1]["content"], "turn-1-assistant");
        assert!(
            messages[2]["content"].as_str().unwrap().contains("omitted"),
            "middle message should be truncation marker"
        );
        assert_eq!(messages[3]["content"], "turn-4-user");
        assert_eq!(messages[4]["content"], "turn-4-assistant-final");
    }

    #[test]
    fn very_tight_budget_keeps_prefix_and_last_turn() {
        let events = vec![
            user("first"),
            text("response-1"),
            user("second"),
            text("response-2"),
            user("third"),
            text("response-3"),
        ];

        // Budget of 1 — forces the "always include last turn" fallback
        let builder = ContextWindowBuilder::new(1);
        let messages = builder.build(&events);

        // Should have prefix (turn 1) + marker + last turn (turn 3)
        assert!(
            messages.len() >= 3,
            "should have at least prefix + marker + tail"
        );

        // First message should be from turn 1
        assert_eq!(messages[0]["content"], "first");

        // Last messages should be from the final turn
        let last = messages.last().unwrap();
        assert_eq!(last["content"], "response-3");
    }

    #[test]
    fn single_turn_always_returned() {
        let events = vec![user("hello"), text("hi there")];

        // Even with a tiny budget, the single turn must be returned
        let builder = ContextWindowBuilder::new(1);
        let messages = builder.build(&events);
        assert_eq!(messages.len(), 2);
    }

    #[test]
    fn tool_calls_preserved_through_truncation() {
        let events = vec![
            // Turn 1: simple exchange
            user("turn 1"),
            text("response 1"),
            // Turn 2: with tool calls (will be dropped)
            user("turn 2"),
            text("checking"),
            tool_call("tc1", "suggest_command"),
            tool_result("tc1", "output"),
            text("done"),
            // Turn 3: final turn (kept in tail)
            user("turn 3"),
            text("final response"),
        ];

        // Budget that fits turn 1 + turn 3 + marker but not turn 2
        let turn1 = events_to_messages(&events[0..2]);
        let turn3 = events_to_messages(&events[7..9]);
        let marker_cost = estimate_chars(std::slice::from_ref(&truncation_marker()));
        let budget = estimate_chars(&turn1) + estimate_chars(&turn3) + marker_cost + 10;

        let builder = ContextWindowBuilder::new(budget);
        let messages = builder.build(&events);

        // Verify turn 2 (the tool call turn) was dropped
        let has_tool_use = messages.iter().any(|m| {
            m["content"]
                .as_array()
                .is_some_and(|arr| arr.iter().any(|b| b["type"] == "tool_use"))
        });
        assert!(!has_tool_use, "tool call turn should have been truncated");

        // Verify first and last turns present
        assert_eq!(messages[0]["content"], "turn 1");
        assert_eq!(messages.last().unwrap()["content"], "final response");
    }

    #[test]
    fn tail_accumulates_multiple_turns_when_budget_allows() {
        // Use long content so turn sizes dwarf the truncation marker.
        let padding = "x".repeat(500);
        let events = vec![
            user(&format!("turn-1-user-{padding}")),
            text(&format!("turn-1-response-{padding}")),
            user(&format!("turn-2-user-{padding}")),
            text(&format!("turn-2-response-{padding}")),
            user(&format!("turn-3-user-{padding}")),
            text(&format!("turn-3-response-{padding}")),
            user(&format!("turn-4-user-{padding}")),
            text(&format!("turn-4-response-{padding}")),
        ];

        // Budget that fits everything except turn 2
        let all = events_to_messages(&events);
        let total = estimate_chars(&all);
        let turn2 = events_to_messages(&events[2..4]);
        let turn2_chars = estimate_chars(&turn2);

        let marker_cost = estimate_chars(std::slice::from_ref(&truncation_marker()));
        let budget = total - turn2_chars + marker_cost + 5;
        assert!(
            budget < total,
            "budget must be less than total for truncation to trigger"
        );

        let builder = ContextWindowBuilder::new(budget);
        let messages = builder.build(&events);

        // Should have: prefix (t1: 2 msgs) + marker (1 msg) + t3 (2 msgs) + t4 (2 msgs) = 7
        // (turn 2 dropped)
        assert_eq!(messages.len(), 7);
        assert!(
            messages[0]["content"]
                .as_str()
                .unwrap()
                .starts_with("turn-1-user-")
        );
        assert!(
            messages[1]["content"]
                .as_str()
                .unwrap()
                .starts_with("turn-1-response-")
        );
        assert!(messages[2]["content"].as_str().unwrap().contains("omitted"));
        assert!(
            messages[3]["content"]
                .as_str()
                .unwrap()
                .starts_with("turn-3-user-")
        );
        assert!(
            messages[4]["content"]
                .as_str()
                .unwrap()
                .starts_with("turn-3-response-")
        );
        assert!(
            messages[5]["content"]
                .as_str()
                .unwrap()
                .starts_with("turn-4-user-")
        );
        assert!(
            messages[6]["content"]
                .as_str()
                .unwrap()
                .starts_with("turn-4-response-")
        );
    }

    #[test]
    fn no_marker_when_no_turns_dropped() {
        // Two turns, both fit in budget
        let events = vec![user("a"), text("b"), user("c"), text("d")];

        let builder = ContextWindowBuilder::with_default_budget();
        let messages = builder.build(&events);

        // No truncation marker
        assert_eq!(messages.len(), 4);
        assert!(
            !messages
                .iter()
                .any(|m| m["content"].as_str().is_some_and(|s| s.contains("omitted")))
        );
    }

    #[test]
    fn tool_use_and_tool_result_never_split() {
        // Invariant: a tool_use and its matching tool_result must always
        // end up in the same turn, so truncation can't orphan one from
        // the other. This test verifies that ToolResult does NOT start
        // a new turn boundary.
        let padding = "x".repeat(500);
        let events = vec![
            // Turn 1 (prefix)
            user(&format!("turn-1-{padding}")),
            text(&format!("resp-1-{padding}")),
            // Turn 2: contains a tool_use → tool_result pair (will be dropped)
            user(&format!("turn-2-{padding}")),
            text("checking"),
            tool_call("tc1", "suggest_command"),
            tool_result("tc1", &format!("output-{padding}")),
            text(&format!("done-{padding}")),
            // Turn 3 (tail)
            user(&format!("turn-3-{padding}")),
            text(&format!("resp-3-{padding}")),
        ];

        // Budget that fits turn 1 + turn 3 + marker, but not turn 2
        let turn1 = events_to_messages(&events[0..2]);
        let turn3 = events_to_messages(&events[7..9]);
        let marker_cost = estimate_chars(std::slice::from_ref(&truncation_marker()));
        let budget = estimate_chars(&turn1) + estimate_chars(&turn3) + marker_cost + 10;

        let builder = ContextWindowBuilder::new(budget);
        let messages = builder.build(&events);

        // Verify: every tool_use has a matching tool_result, and vice versa
        let tool_use_ids: Vec<&str> = messages
            .iter()
            .filter_map(|m| m["content"].as_array())
            .flatten()
            .filter(|b| b["type"] == "tool_use")
            .filter_map(|b| b["id"].as_str())
            .collect();

        let tool_result_ids: Vec<&str> = messages
            .iter()
            .filter_map(|m| m["content"].as_array())
            .flatten()
            .filter(|b| b["type"] == "tool_result")
            .filter_map(|b| b["tool_use_id"].as_str())
            .collect();

        assert_eq!(
            tool_use_ids, tool_result_ids,
            "every tool_use must have a matching tool_result (and vice versa)"
        );

        // Turn 2 was dropped entirely, so no tool IDs should be present
        assert!(
            !tool_use_ids.contains(&"tc1"),
            "dropped turn's tool_use should not appear"
        );
    }
}
