//! The picker's query language: free text plus filter tokens.
//!
//! - `agent:claude` (or `a:claude`; the agent as `claude-code`, `cc`, `codex`, `oc`, `pi`, ...):
//!   only this agent's sessions;
//! - `m:opus`: model contains this;
//! - `b:main`: on this git branch.
//!
//! Tokens are whitespace-separated words anywhere in the input. They are parsed out before the
//! rest runs as the full-text query, and rendered as chips in the input box. A leading `\` makes a
//! word literal text (`\b:main`).

use std::ops::Range;

use atuin_client::ai_session::HarnessKind;

use crate::commands::session::harness_name;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenKind {
    Harness,
    Model,
    Branch,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenState {
    /// Applied as a filter.
    Valid,
    /// Still being typed (`agent:`): shown as a chip, not applied yet.
    Pending,
    /// Not understood (`agent:vim`): shown as an error chip, ignored.
    Invalid,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Token {
    pub kind: TokenKind,
    /// Byte range of the whole word in the input.
    pub range: Range<usize>,
    pub state: TokenState,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ParsedQuery {
    /// The free-text words, single-space separated, and ending in a space when the input does:
    /// a trailing space finishes the last word, which then no longer matches as a prefix.
    pub text: String,
    pub harness: Option<HarnessKind>,
    pub model: Option<String>,
    pub branch: Option<String>,
    /// Every token word, in input order.
    pub tokens: Vec<Token>,
}

/// Parse a harness name or its badge.
pub fn parse_harness(s: &str) -> Option<HarnessKind> {
    match s.to_ascii_lowercase().as_str() {
        "claude" | "claude-code" | "claudecode" | "cc" => Some(HarnessKind::ClaudeCode),
        "codex" | "cx" => Some(HarnessKind::Codex),
        "opencode" | "oc" => Some(HarnessKind::Opencode),
        "pi" => Some(HarnessKind::Pi),
        _ => None,
    }
}

/// The harnesses alt-a cycles through, after "all".
const HARNESS_CYCLE: [HarnessKind; 4] =
    [HarnessKind::ClaudeCode, HarnessKind::Codex, HarnessKind::Opencode, HarnessKind::Pi];

/// Whitespace-separated words with their byte ranges.
fn words(input: &str) -> impl Iterator<Item = (Range<usize>, &str)> {
    let mut start = None;
    let mut out = Vec::new();
    for (i, c) in input.char_indices() {
        match (c.is_whitespace(), start) {
            (true, Some(s)) => {
                out.push(s..i);
                start = None;
            }
            (false, None) => start = Some(i),
            _ => {}
        }
    }
    if let Some(s) = start {
        out.push(s..input.len());
    }
    out.into_iter().map(move |r| (r.clone(), &input[r]))
}

/// Classify a word as a token: its kind and value, or `None` for plain text.
fn classify(word: &str) -> Option<(TokenKind, &str)> {
    if let Some(v) = word.strip_prefix("agent:").or_else(|| word.strip_prefix("a:")) {
        Some((TokenKind::Harness, v))
    } else if let Some(v) = word.strip_prefix("m:") {
        Some((TokenKind::Model, v))
    } else {
        word.strip_prefix("b:").map(|v| (TokenKind::Branch, v))
    }
}

pub fn parse(input: &str) -> ParsedQuery {
    let mut parsed = ParsedQuery::default();
    let mut text = Vec::new();

    for (range, word) in words(input) {
        if let Some(literal) = word.strip_prefix('\\') {
            if !literal.is_empty() {
                text.push(literal);
            }
            continue;
        }
        let Some((kind, value)) = classify(word) else {
            text.push(word);
            continue;
        };

        let state = if value.is_empty() {
            TokenState::Pending
        } else {
            match kind {
                TokenKind::Harness => match parse_harness(value) {
                    Some(h) => {
                        parsed.harness = Some(h);
                        TokenState::Valid
                    }
                    None => TokenState::Invalid,
                },
                TokenKind::Model => {
                    parsed.model = Some(value.to_owned());
                    TokenState::Valid
                }
                TokenKind::Branch => {
                    parsed.branch = Some(value.to_owned());
                    TokenState::Valid
                }
            }
        };
        parsed.tokens.push(Token { kind, range, state });
    }

    parsed.text = text.join(" ");
    if !parsed.text.is_empty() && input.ends_with(char::is_whitespace) {
        parsed.text.push(' ');
    }
    parsed
}

/// alt-a: advance the input's agent token through all → claude-code → codex → opencode → pi → all.
///
/// Rewrites the first agent token (`agent:` or `a:`) in place as `agent:<name>` (removing it for
/// "all"), or appends one.
pub fn cycle_harness(input: &str) -> String {
    let parsed = parse(input);
    let existing = parsed.tokens.iter().find(|t| t.kind == TokenKind::Harness);

    let next = match (existing, parsed.harness) {
        (Some(_), Some(one)) => {
            let i = HARNESS_CYCLE.iter().position(|h| *h == one);
            i.and_then(|i| HARNESS_CYCLE.get(i + 1)).copied()
        }
        _ => Some(HARNESS_CYCLE[0]),
    };
    let replacement = next.map(|h| format!("agent:{}", harness_name(h)));

    match (existing, replacement) {
        (Some(token), Some(rep)) => {
            format!("{}{rep}{}", &input[..token.range.start], &input[token.range.end..])
        }
        (Some(token), None) => {
            let before = input[..token.range.start].trim_end();
            let after = input[token.range.end..].trim_start();
            match (before.is_empty(), after.is_empty()) {
                (true, _) => after.to_owned(),
                (_, true) => before.to_owned(),
                _ => format!("{before} {after}"),
            }
        }
        (None, Some(rep)) if input.trim().is_empty() => rep,
        (None, Some(rep)) if input.ends_with(char::is_whitespace) => format!("{input}{rep}"),
        (None, Some(rep)) => format!("{input} {rep}"),
        (None, None) => input.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[rstest]
    fn plain_text_is_the_query() {
        let q = parse("  fix the   flaky test");
        assert_eq!(q.text, "fix the flaky test");
        assert!(q.tokens.is_empty());
        assert!(q.harness.is_none());
    }

    /// A trailing space finishes the last word: the search then matches it whole, not as a
    /// prefix (`refac ` doesn't find `refactor`).
    #[rstest]
    #[case::finished("refac ", "refac ")]
    #[case::finished_by_any_whitespace("fix the  refac\t", "fix the refac ")]
    #[case::still_typing("refac", "refac")]
    #[case::after_a_token("refac b:main ", "refac ")]
    #[case::only_tokens("b:main ", "")]
    #[case::only_space("   ", "")]
    fn a_trailing_space_finishes_the_last_word(#[case] input: &str, #[case] text: &str) {
        assert_eq!(parse(input).text, text);
    }

    #[rstest]
    fn tokens_are_parsed_out_anywhere() {
        let input = "agent:claude flaky m:opus test b:main @build";
        let q = parse(input);
        assert_eq!(q.text, "flaky test @build");
        assert_eq!(q.harness, Some(HarnessKind::ClaudeCode));
        assert_eq!(q.model.as_deref(), Some("opus"));
        assert_eq!(q.branch.as_deref(), Some("main"));
        let kinds: Vec<_> = q.tokens.iter().map(|t| t.kind).collect();
        assert_eq!(kinds, vec![TokenKind::Harness, TokenKind::Model, TokenKind::Branch]);
        assert_eq!(&input[q.tokens[1].range.clone()], "m:opus");
        assert!(q.tokens.iter().all(|t| t.state == TokenState::Valid));
    }

    #[rstest]
    #[case("agent:cc", Some(HarnessKind::ClaudeCode))]
    #[case("agent:claude-code", Some(HarnessKind::ClaudeCode))]
    #[case("agent:cx", Some(HarnessKind::Codex))]
    #[case("agent:oc", Some(HarnessKind::Opencode))]
    #[case("agent:codex agent:pi", Some(HarnessKind::Pi))]
    #[case("agent:PI", Some(HarnessKind::Pi))]
    #[case("agent:codex,pi", None)]
    #[case("a:cc", Some(HarnessKind::ClaudeCode))]
    #[case("a:codex", Some(HarnessKind::Codex))]
    #[case("a:opencode", Some(HarnessKind::Opencode))]
    #[case("A:pi", None)]
    #[case("agent:codex a:pi", Some(HarnessKind::Pi))]
    #[case("a:pi agent:claude", Some(HarnessKind::ClaudeCode))]
    fn harness_tokens(#[case] input: &str, #[case] want: Option<HarnessKind>) {
        assert_eq!(parse(input).harness, want);
    }

    /// `a:` is the short form of `agent:`: the same token, over the same range.
    #[rstest]
    #[case("agent:codex")]
    #[case("a:codex")]
    fn agent_and_its_short_form_parse_the_same(#[case] word: &str) {
        let input = format!("bug {word}");
        let q = parse(&input);
        assert_eq!(q.harness, Some(HarnessKind::Codex));
        assert_eq!(q.text, "bug");
        assert_eq!(q.tokens, vec![Token {
            kind: TokenKind::Harness,
            range: 4..input.len(),
            state: TokenState::Valid,
        }]);
    }

    /// `h:` was the agent token before `agent:`; it's plain text now.
    #[rstest]
    fn h_is_no_longer_a_token() {
        let q = parse("h:codex bug");
        assert!(q.tokens.is_empty());
        assert!(q.harness.is_none());
        assert_eq!(q.text, "h:codex bug");
    }

    #[rstest]
    fn unknown_harness_is_an_invalid_token_and_ignored() {
        let q = parse("agent:vim bug");
        assert_eq!(q.tokens[0].state, TokenState::Invalid);
        assert!(q.harness.is_none());
        assert_eq!(q.text, "bug");
    }

    #[rstest]
    fn empty_values_are_pending() {
        let q = parse("bug agent: b: a:");
        assert_eq!(q.tokens.len(), 3);
        assert!(q.tokens.iter().all(|t| t.state == TokenState::Pending));
        assert!(q.branch.is_none());
        assert_eq!(q.text, "bug");
    }

    #[rstest]
    fn backslash_makes_a_word_literal() {
        let q = parse(r"\b:home \agent:x \a:y");
        assert!(q.tokens.is_empty());
        assert_eq!(q.text, "b:home agent:x a:y");
    }

    #[rstest]
    fn later_scalar_tokens_win() {
        let q = parse("m:sonnet m:opus");
        assert_eq!(q.model.as_deref(), Some("opus"));
    }

    #[rstest]
    fn multibyte_ranges_are_char_boundaries() {
        let input = "héllo b:hôst";
        let q = parse(input);
        assert_eq!(&input[q.tokens[0].range.clone()], "b:hôst");
        assert_eq!(q.branch.as_deref(), Some("hôst"));
    }

    #[rstest]
    #[case("", "agent:claude-code")]
    #[case("flaky", "flaky agent:claude-code")]
    #[case("flaky ", "flaky agent:claude-code")]
    #[case("agent:claude flaky", "agent:codex flaky")]
    #[case("flaky agent:codex", "flaky agent:opencode")]
    #[case("agent:opencode", "agent:pi")]
    #[case("agent:pi", "")]
    #[case("a agent:pi b", "a b")]
    #[case("flaky agent:pi", "flaky")]
    #[case("agent:codex,pi x", "agent:claude-code x")]
    #[case("agent:vim", "agent:claude-code")]
    #[case("a:claude flaky", "agent:codex flaky")]
    #[case("flaky a:cx", "flaky agent:opencode")]
    #[case("a:pi", "")]
    #[case("a:", "agent:claude-code")]
    #[case("h:codex", "h:codex agent:claude-code")]
    fn alt_a_cycles_the_harness_token(#[case] input: &str, #[case] want: &str) {
        assert_eq!(cycle_harness(input), want);
    }

    #[rstest]
    fn alt_a_round_trips() {
        let mut s = String::from("bug");
        for _ in 0..5 {
            s = cycle_harness(&s);
        }
        assert_eq!(s, "bug");
    }
}
