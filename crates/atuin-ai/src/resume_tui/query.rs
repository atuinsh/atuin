//! The picker's query language: free text plus filter tokens.
//!
//! - `h:claude` (or `h:cc`, `h:codex,pi`): only these harnesses;
//! - `m:opus`: model contains this;
//! - `b:main`: on this git branch;
//! - `@buildbox`: recorded on a host whose name starts with this.
//!
//! Tokens are whitespace-separated words anywhere in the input. They are parsed out before the
//! rest runs as the full-text query, and rendered as chips in the input box. A leading `\` makes a
//! word literal text (`\@home`).

use std::ops::Range;

use atuin_client::ai_session::HarnessKind;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenKind {
    Harness,
    Model,
    Branch,
    Host,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenState {
    /// Applied as a filter.
    Valid,
    /// Still being typed (`h:`, `@`): shown as a chip, not applied yet.
    Pending,
    /// Not understood (`h:vim`): shown as an error chip, ignored.
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
    /// The free-text words, single-space separated.
    pub text: String,
    pub harnesses: Vec<HarnessKind>,
    pub model: Option<String>,
    pub branch: Option<String>,
    pub host: Option<String>,
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

/// The name `h:` tokens are written with.
pub fn harness_token_name(harness: HarnessKind) -> &'static str {
    match harness {
        HarnessKind::ClaudeCode => "claude",
        HarnessKind::Codex => "codex",
        HarnessKind::Opencode => "opencode",
        HarnessKind::Pi => "pi",
        HarnessKind::Copilot => "copilot",
        HarnessKind::Unknown => "unknown",
    }
}

/// The harnesses alt-h cycles through, after "all".
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
    if let Some(v) = word.strip_prefix("h:") {
        Some((TokenKind::Harness, v))
    } else if let Some(v) = word.strip_prefix("m:") {
        Some((TokenKind::Model, v))
    } else if let Some(v) = word.strip_prefix("b:") {
        Some((TokenKind::Branch, v))
    } else {
        word.strip_prefix('@').map(|v| (TokenKind::Host, v))
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
                TokenKind::Harness => {
                    let harnesses: Option<Vec<_>> = value
                        .split([',', '|'])
                        .filter(|v| !v.is_empty())
                        .map(parse_harness)
                        .collect();
                    match harnesses {
                        Some(hs) if !hs.is_empty() => {
                            for h in hs {
                                if !parsed.harnesses.contains(&h) {
                                    parsed.harnesses.push(h);
                                }
                            }
                            TokenState::Valid
                        }
                        _ => TokenState::Invalid,
                    }
                }
                TokenKind::Model => {
                    parsed.model = Some(value.to_owned());
                    TokenState::Valid
                }
                TokenKind::Branch => {
                    parsed.branch = Some(value.to_owned());
                    TokenState::Valid
                }
                TokenKind::Host => {
                    parsed.host = Some(value.to_owned());
                    TokenState::Valid
                }
            }
        };
        parsed.tokens.push(Token { kind, range, state });
    }

    parsed.text = text.join(" ");
    parsed
}

/// alt-h: advance the input's `h:` token through all → claude → codex → opencode → pi → all.
///
/// Rewrites the first `h:` token in place (removing it for "all"), or appends one.
pub fn cycle_harness(input: &str) -> String {
    let parsed = parse(input);
    let existing = parsed.tokens.iter().find(|t| t.kind == TokenKind::Harness);

    let next = match (existing, parsed.harnesses.as_slice()) {
        (Some(_), [one]) => {
            let i = HARNESS_CYCLE.iter().position(|h| h == one);
            i.and_then(|i| HARNESS_CYCLE.get(i + 1)).copied()
        }
        _ => Some(HARNESS_CYCLE[0]),
    };
    let replacement = next.map(|h| format!("h:{}", harness_token_name(h)));

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

    #[test]
    fn plain_text_is_the_query() {
        let q = parse("  fix the   flaky test ");
        assert_eq!(q.text, "fix the flaky test");
        assert!(q.tokens.is_empty());
        assert!(q.harnesses.is_empty());
    }

    #[test]
    fn tokens_are_parsed_out_anywhere() {
        let input = "h:claude flaky m:opus test b:main @build";
        let q = parse(input);
        assert_eq!(q.text, "flaky test");
        assert_eq!(q.harnesses, vec![HarnessKind::ClaudeCode]);
        assert_eq!(q.model.as_deref(), Some("opus"));
        assert_eq!(q.branch.as_deref(), Some("main"));
        assert_eq!(q.host.as_deref(), Some("build"));
        let kinds: Vec<_> = q.tokens.iter().map(|t| t.kind).collect();
        assert_eq!(kinds, vec![
            TokenKind::Harness,
            TokenKind::Model,
            TokenKind::Branch,
            TokenKind::Host
        ]);
        assert_eq!(&input[q.tokens[1].range.clone()], "m:opus");
        assert!(q.tokens.iter().all(|t| t.state == TokenState::Valid));
    }

    #[rstest]
    #[case("h:cc", vec![HarnessKind::ClaudeCode])]
    #[case("h:claude-code", vec![HarnessKind::ClaudeCode])]
    #[case("h:codex,pi", vec![HarnessKind::Codex, HarnessKind::Pi])]
    #[case("h:oc|cx", vec![HarnessKind::Opencode, HarnessKind::Codex])]
    #[case("h:pi h:pi", vec![HarnessKind::Pi])]
    #[case("h:PI", vec![HarnessKind::Pi])]
    fn harness_tokens(#[case] input: &str, #[case] want: Vec<HarnessKind>) {
        assert_eq!(parse(input).harnesses, want);
    }

    #[test]
    fn unknown_harness_is_an_invalid_token_and_ignored() {
        let q = parse("h:vim bug");
        assert_eq!(q.tokens[0].state, TokenState::Invalid);
        assert!(q.harnesses.is_empty());
        assert_eq!(q.text, "bug");
    }

    #[test]
    fn empty_values_are_pending() {
        let q = parse("bug h: @");
        assert_eq!(q.tokens.len(), 2);
        assert!(q.tokens.iter().all(|t| t.state == TokenState::Pending));
        assert!(q.host.is_none());
        assert_eq!(q.text, "bug");
    }

    #[test]
    fn backslash_makes_a_word_literal() {
        let q = parse(r"\@home \h:x");
        assert!(q.tokens.is_empty());
        assert_eq!(q.text, "@home h:x");
    }

    #[test]
    fn later_scalar_tokens_win() {
        let q = parse("m:sonnet m:opus");
        assert_eq!(q.model.as_deref(), Some("opus"));
    }

    #[test]
    fn multibyte_ranges_are_char_boundaries() {
        let input = "héllo @hôst";
        let q = parse(input);
        assert_eq!(&input[q.tokens[0].range.clone()], "@hôst");
        assert_eq!(q.host.as_deref(), Some("hôst"));
    }

    #[rstest]
    #[case("", "h:claude")]
    #[case("flaky", "flaky h:claude")]
    #[case("flaky ", "flaky h:claude")]
    #[case("h:claude flaky", "h:codex flaky")]
    #[case("flaky h:codex", "flaky h:opencode")]
    #[case("h:opencode", "h:pi")]
    #[case("h:pi", "")]
    #[case("a h:pi b", "a b")]
    #[case("flaky h:pi", "flaky")]
    #[case("h:codex,pi x", "h:claude x")]
    #[case("h:vim", "h:claude")]
    fn alt_h_cycles_the_harness_token(#[case] input: &str, #[case] want: &str) {
        assert_eq!(cycle_harness(input), want);
    }

    #[test]
    fn alt_h_round_trips() {
        let mut s = String::from("bug");
        for _ in 0..5 {
            s = cycle_harness(&s);
        }
        assert_eq!(s, "bug");
    }
}
