//! A title for a session its harness didn't title, from its first prompt.
//!
//! Prompts open with a lot that isn't what the session is about: an agent's brief starts with
//! who it is (`You're working on … in a git worktree of …`), where things are (`Repo: /…`) and
//! what to read first; a person's may start with a greeting or a pasted system block. The title
//! is the first sentence that isn't one of those, stripped of its markdown, on one line and cut
//! to [`MAX`] columns. When every sentence is, the role an agent was given is the best there is
//! (`You are a code-review verifier.` gives `Code-review verifier`).
//!
//! Cheap and deterministic: rows derive it once, as they are listed.

use std::ops::Range;

use atuin_common::string::ellipsis::{Indicator, Pos};
use atuin_common::string::{EllipsizeExt as _, Measure};

/// The longest a derived title gets, in columns.
pub const MAX: usize = 80;

/// How many sentences to look through for one that isn't preamble.
const LOOK_AT: usize = 8;

/// A title for a session from its first prompt; empty when there is nothing to go on.
pub fn derive(prompt: &str) -> String {
    let body = strip_tags(prompt);
    // An explicit `Title: …` line near the top names the session outright.
    if let Some(title) = body
        .lines()
        .take(LOOK_AT * 2)
        .find_map(|l| l.trim().strip_prefix("Title:"))
        .map(clean)
        .filter(|t| !t.is_empty())
    {
        return cap(&title);
    }
    let sentences: Vec<String> =
        sentences(&body).take(LOOK_AT).map(|s| clean(&s)).filter(|s| !s.is_empty()).collect();
    let chosen = sentences
        .iter()
        .find_map(|s| match kind(s) {
            Kind::Topic(t) => Some(t),
            Kind::Role(_) | Kind::Preamble => None,
        })
        .or_else(|| {
            sentences.iter().find_map(|s| match kind(s) {
                Kind::Role(r) => Some(r),
                _ => None,
            })
        })
        .or_else(|| sentences.first().cloned())
        .unwrap_or_default();
    cap(&chosen)
}

/// Byte ranges of `text` matching the query's words, ignoring ASCII case, for highlighting a
/// derived title (the search highlights stored titles itself). Matches start at a word.
pub fn highlights(text: &str, query: &str) -> Vec<Range<usize>> {
    let lower = text.to_ascii_lowercase();
    let ranges: Vec<Range<usize>> = query
        .split_whitespace()
        .map(|w| w.trim_matches('"').to_ascii_lowercase())
        .filter(|w| !w.is_empty())
        .flat_map(|w| {
            let lower = &lower;
            lower
                .match_indices(w.as_str())
                .filter(|(i, _)| {
                    lower[..*i].chars().next_back().is_none_or(|c| !c.is_alphanumeric())
                })
                .map(|(i, m)| i..i + m.len())
                .collect::<Vec<_>>()
        })
        .collect();
    merge(ranges)
}

/// `ranges` sorted, with those that overlap or touch joined into one.
pub fn merge(mut ranges: Vec<Range<usize>>) -> Vec<Range<usize>> {
    ranges.sort_by_key(|r| r.start);
    let mut merged: Vec<Range<usize>> = Vec::new();
    for r in ranges {
        match merged.last_mut() {
            Some(last) if r.start <= last.end => last.end = last.end.max(r.end),
            _ => merged.push(r),
        }
    }
    merged
}

enum Kind {
    /// What the session is about.
    Topic(String),
    /// Who an agent was told it is: `Code-review verifier`.
    Role(String),
    /// Setup: where things are, what to read first, rules, a greeting.
    Preamble,
}

/// Openings that set an agent up rather than say what to do.
const PREAMBLE: &[&str] = &[
    "read ",
    "first read",
    "background",
    "context",
    "base on ",
    "make sure you",
    "create and commit",
    "the code is in",
    // A code-review agent's brief.
    "repo:",
    "repo (",
    "repo /",
    "diff:",
    "diff (",
    "files:",
    "changed:",
    "change summary",
    "scope:",
    "scope =",
    "note:",
    "important",
    "hard rules",
    "rules:",
    "read-only",
    "search breadth",
    "very thorough",
    "caveat:",
];

/// Openings dropped from the front of a sentence that goes on to say something.
const FILLER: &[&str] = &[
    "please ",
    "hi, ",
    "hi ",
    "hello, ",
    "hello ",
    "hey, ",
    "hey ",
    "ok, ",
    "okay, ",
    "so, ",
    "can you ",
    "could you ",
];

const GREETINGS: &[&str] = &[
    "hi",
    "hello",
    "hey",
    "heya",
    "hiya",
    "yo",
    "yoyo",
    "yoyoyo",
    "sup",
    "morning",
    "good morning",
    "hi there",
    "hey there",
    "thanks",
];

fn kind(sentence: &str) -> Kind {
    let lower = sentence.to_lowercase();
    for role in ["you are ", "you're ", "you’re "] {
        if let Some(rest) = strip_prefix_ci(sentence, role) {
            // `You're implementing X` says what to do, and `You are a code-review verifier`
            // who the session is: both name it better than the brief's details do. `You're
            // working on X` and `You're in /path` only set the scene.
            let verb = rest.split_whitespace().next().unwrap_or_default();
            if verb.len() > 4 && verb.ends_with("ing") && verb != "working" {
                return Kind::Topic(capitalise(rest));
            }
            if let Some(who) = ["a ", "an ", "the "].iter().find_map(|a| strip_prefix_ci(rest, a)) {
                return Kind::Topic(capitalise(who));
            }
            if strip_prefix_ci(rest, "one ").is_some() {
                return Kind::Topic(capitalise(rest));
            }
            return Kind::Role(capitalise(rest));
        }
    }
    let bare = lower.trim_end_matches(|c: char| !c.is_alphanumeric());
    if GREETINGS.contains(&bare) || bare.chars().all(|c| !c.is_alphanumeric()) {
        return Kind::Preamble;
    }
    if PREAMBLE.iter().any(|p| lower.starts_with(p)) || is_label(sentence) || is_paths(sentence) {
        return Kind::Preamble;
    }
    // A lone word (`Verifier.`) says little; something after it may say more.
    if !sentence.contains(char::is_whitespace) && !sentence.starts_with(['/', '$']) {
        return Kind::Role(capitalise(sentence));
    }
    let mut topic = sentence;
    while let Some(rest) = FILLER.iter().find_map(|f| strip_prefix_ci(topic, f)) {
        topic = rest;
    }
    if topic.len() == sentence.len() {
        Kind::Topic(sentence.to_owned())
    } else {
        Kind::Topic(capitalise(topic))
    }
}

/// `Repo: /path`, `Diff (uncommitted): /path`, `Atuin CLI: /path (Rust)`: a short label for a
/// path or link.
fn is_label(sentence: &str) -> bool {
    let Some((label, value)) = sentence.split_once(": ") else {
        return false;
    };
    let value = value.trim_start();
    label.split_whitespace().count() <= 3
        && (value.starts_with(['/', '~', '.']) || value.contains("://"))
}

/// Mostly paths or links: `Scope = uncommitted diff at /private/tmp/…/review.diff`.
fn is_paths(sentence: &str) -> bool {
    let paths: usize = sentence
        .split_whitespace()
        .filter(|w| {
            w.trim_start_matches(['(', '`', '"']).starts_with(['/', '~']) || w.contains("://")
        })
        .map(str::len)
        .sum();
    paths * 2 >= sentence.len()
}

/// `s` without `prefix`, ignoring ASCII case.
fn strip_prefix_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    s.get(..prefix.len()).filter(|p| p.eq_ignore_ascii_case(prefix)).map(|_| &s[prefix.len()..])
}

fn capitalise(s: &str) -> String {
    let mut chars = s.chars();
    chars.next().map_or_else(String::new, |c| c.to_uppercase().chain(chars).collect())
}

/// `text` without the XML-ish blocks some prompts open with (`<system_instruction>…</…>`,
/// `<image name=…>`): a block that closes is dropped whole, a tag that doesn't is dropped alone.
fn strip_tags(text: &str) -> String {
    let mut rest = text.trim_start();
    while let Some(after) = rest.strip_prefix('<') {
        let Some(end) = after.find('>') else {
            break;
        };
        let tag = &after[..end];
        let name = tag.split_whitespace().next().unwrap_or("");
        if name.is_empty() || !name.chars().all(|c| c.is_alphanumeric() || "-_:".contains(c)) {
            break;
        }
        let after_tag = &after[end + 1..];
        let close = format!("</{name}>");
        rest = match after_tag.find(&close) {
            Some(at) => &after_tag[at + close.len()..],
            None => after_tag,
        }
        .trim_start();
    }
    rest.to_owned()
}

/// The sentences of `text`, in order: each line is split after `. `, `? ` or `! `, and blank
/// lines, fences and table rules are skipped.
fn sentences(text: &str) -> impl Iterator<Item = String> + '_ {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with("```") && !l.starts_with("|-"))
        .flat_map(|line| {
            let mut out = Vec::new();
            let mut start = 0;
            let bytes = line.as_bytes();
            for (i, c) in line.char_indices() {
                // Not after a single letter, as in `e.g. ` or `A. `.
                let initial = || {
                    line[..i]
                        .rsplit(|c: char| !c.is_alphanumeric())
                        .next()
                        .is_some_and(|w| w.chars().count() == 1)
                };
                if matches!(c, '.' | '?' | '!')
                    && bytes.get(i + 1).is_some_and(|b| *b == b' ')
                    && !initial()
                {
                    out.push(line[start..=i].to_owned());
                    start = i + 2;
                }
            }
            if start < line.len() {
                out.push(line[start..].to_owned());
            }
            out
        })
}

/// One sentence as plain text on one line: markdown markers and link targets dropped,
/// whitespace collapsed, and a trailing `.` or `:` taken off.
fn clean(sentence: &str) -> String {
    let mut s = sentence.trim();
    // Line-leading markdown: headings, quotes, bullets, numbered items.
    loop {
        let before = s;
        s = s.trim_start_matches('#').trim_start_matches('>').trim_start();
        for bullet in ["- ", "* ", "+ ", "• "] {
            s = s.strip_prefix(bullet).unwrap_or(s);
        }
        if let Some((n, rest)) = s.split_once(". ")
            && !n.is_empty()
            && n.chars().all(|c| c.is_ascii_digit())
        {
            s = rest;
        }
        if s == before {
            break;
        }
    }
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '`' => {}
            // `**bold**`, `__bold__`; single `_` stays (snake_case).
            '*' => {}
            '_' if chars.peek() == Some(&'_') => {
                chars.next();
            }
            // `[text](url)` keeps the text; `![alt](url)` too.
            '!' if chars.peek() == Some(&'[') => {}
            ']' if chars.peek() == Some(&'(') => {
                for c in chars.by_ref() {
                    if c == ')' {
                        break;
                    }
                }
            }
            '[' => {}
            c if c.is_whitespace() || c.is_control() => {
                if !out.ends_with(' ') {
                    out.push(' ');
                }
            }
            c => out.push(c),
        }
    }
    out.trim().trim_end_matches(['.', ':', ',', ';']).trim_end().to_owned()
}

fn cap(title: &str) -> String {
    title.ellipsize(Measure::Columns(MAX), Pos::End, Indicator::UNICODE).to_string()
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    /// Prompts as they come: agent briefs, questions, pasted blocks, markdown.
    #[rstest]
    #[case::plain(
        "Fix the flaky sync test in the record store",
        "Fix the flaky sync test in the record store"
    )]
    #[case::first_sentence(
        "Do a UX polish pass on the `atuin ai resume` picker. You're in a git worktree of \
         /work/atuin.",
        "Do a UX polish pass on the atuin ai resume picker"
    )]
    #[case::brief_after_setup(
        "You're working on `atuin ai resume` in a git worktree of /work/atuin. Base on the latest \
         `ai-resume`, then create and commit to the branch `ai-resume-ux`. Background is in \
         /tmp/x.md.\n\nThe user wants the preview to render markdown. The session text is…",
        "The user wants the preview to render markdown"
    )]
    #[case::only_setup(
        "You're working on `atuin ai resume` in a git worktree of /work/atuin. Base on the latest \
         `ai-resume`, then create and commit to the branch `ai-resume-ux`.",
        "Working on atuin ai resume in a git worktree of /work/atuin"
    )]
    #[case::role_that_is_the_task(
        "You're implementing per-harness session resume for `atuin ai resume`. First read the \
         agreed design at /tmp/design.md. You're in a git worktree of /work/atuin.",
        "Implementing per-harness session resume for atuin ai resume"
    )]
    #[case::labels_and_rules(
        "You are a VERIFIER in a code review. Repo: /Users/ellie/src/herdr (Rust). Diff \
         (uncommitted): /private/tmp/review.diff. Read-only: do NOT edit repo files; do not run \
         cargo. Verify TWO independent cleanup candidates and give a separate verdict for each.",
        "VERIFIER in a code review"
    )]
    #[case::only_a_role(
        "You are a code-review finder. Repo: /Users/ellie/workspace/hub (Elixir/Phoenix).",
        "Code-review finder"
    )]
    #[case::read_first(
        "Read the design at /tmp/design.md. Atuin CLI: /Users/ellie/workspace/atuin (Rust). Read \
         the code where it helps.\n\nA reviewer (runner lens) claims this security finding:",
        "A reviewer (runner lens) claims this security finding"
    )]
    #[case::title_line(
        "Read the design.\n\nA reviewer claims this finding:\nTitle: Attacker-first redemption of \
         a token\nDetails…",
        "Attacker-first redemption of a token"
    )]
    #[case::system_block(
        "<system_instruction>\nYou are working inside Conductor.\n</system_instruction>\n\nwhy \
         does the daemon crash on startup?",
        "why does the daemon crash on startup?"
    )]
    #[case::unclosed_tag(
        "<fork-boilerplate>\nYou are a worker fork. Execute ONE directive, then stop.",
        "Worker fork"
    )]
    #[case::greeting(
        "hey\n\nplease check our logs and tell me why claude usage errors",
        "Check our logs and tell me why claude usage errors"
    )]
    #[case::greeting_inline("Hi, can you review this diff?", "Review this diff?")]
    #[case::markdown(
        "## Build **atuin ai resume**\n\n- a [picker](https://docs.atuin.sh) over sessions",
        "Build atuin ai resume"
    )]
    #[case::snake_case("Rename `row_count` to `rows`", "Rename row_count to rows")]
    #[case::question_split(
        "What does `git rebase --onto A B C` do? One short paragraph.",
        "What does git rebase --onto A B C do?"
    )]
    #[case::eg_isnt_a_sentence(
        "Use a fixed clock, e.g. a mock. Then rerun.",
        "Use a fixed clock, e.g. a mock"
    )]
    #[case::paths(
        "You are a code-review finder (Angle C: cross-file tracer). Repo: \
         /Users/ellie/workspace/hub (Elixir/Phoenix). The diff under review is at \
         /private/tmp/claude-501/-Users-ellie-hub/7bbc3292/scratchpad/review.diff (also `git diff \
         main...HEAD`).",
        "Code-review finder (Angle C: cross-file tracer)"
    )]
    #[case::a_lone_word(
        "Verifier. Repo /Users/ellie/workspace/atuin/spaces/login. Diff: \
         /private/tmp/review.diff.\n\nCandidate: login sends ForceSync and returns before the \
         sync is done.",
        "Candidate: login sends ForceSync and returns before the sync is done"
    )]
    #[case::yoyoyo(
        "yoyoyo\n\nuse atuin kv to talk to the other codex <3",
        "use atuin kv to talk to the other codex <3"
    )]
    #[case::slash_command("/code-review", "/code-review")]
    #[case::slash_then_text(
        "/fast\n\nwe are building the atuin terminal",
        "we are building the atuin terminal"
    )]
    #[case::greeting_only("say hi", "say hi")]
    #[case::empty("", "")]
    #[case::whitespace("  \n\t ", "")]
    fn derives(#[case] prompt: &str, #[case] want: &str) {
        assert_eq!(derive(prompt), want);
    }

    #[rstest]
    fn long_titles_are_cut() {
        let prompt = "word ".repeat(40);
        let title = derive(&prompt);
        assert_eq!(unicode_width::UnicodeWidthStr::width(title.as_str()), MAX);
        assert!(title.ends_with('…'));
    }

    #[rstest]
    fn highlights_match_word_starts() {
        let text = "Fix the flaky sync test; unflaky";
        let hl = highlights(text, "FLAK sync");
        let words: Vec<&str> = hl.iter().map(|r| &text[r.clone()]).collect();
        assert_eq!(words, ["flak", "sync"]);
        assert!(highlights(text, "").is_empty());
    }
}
