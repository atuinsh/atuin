//! The lines of a highlighted body around each match, numbered the way `GetCommandOutput` numbers
//! them.

use std::ops::Range;

use atuin_client::history::HistoryId;
use atuin_common::string::highlighted::{HighlightedString, HighlightedText};
use easy_cast::Conv;

/// One search hit: the lines around each match in one command's output.
#[derive(Debug)]
pub struct OutputMatch {
    pub history_id: HistoryId,
    /// Ascending; a gap in `line` between neighbours is output that was not shown.
    pub lines: Vec<OutputLine>,
    /// Relevance -- higher is more relevant.
    pub score: f64,
}

#[derive(Debug)]
pub struct OutputLine {
    /// The line's position in the output: 0-based from the start, or negative -- counting back
    /// from the end -- for a line of the kept tail of an output whose middle was discarded.
    pub line: i64,
    pub content: HighlightedString,
}

/// The lines of `body` within `context` lines of a match. `tail_from` is the index of the first
/// line of the kept tail when the output's middle was discarded; those are numbered from the end.
pub fn snippet<S: AsRef<str>>(
    body: &HighlightedText<S>,
    tail_from: Option<usize>,
    context: usize,
) -> Vec<OutputLine> {
    /// Coalesce ascending ranges that overlap or touch.
    fn merge(ranges: impl Iterator<Item = Range<usize>>) -> Vec<Range<usize>> {
        ranges.fold(Vec::new(), |mut merged, r| {
            match merged.last_mut() {
                Some(last) if r.start <= last.end => last.end = last.end.max(r.end),
                _ => merged.push(r),
            }
            merged
        })
    }

    let lines: Vec<HighlightedText<&str>> = body.lines().collect();
    let number = |idx: usize| match tail_from {
        Some(tail) if idx >= tail => i64::conv(idx) - i64::conv(lines.len()),
        _ => i64::conv(idx),
    };
    let windows = merge(
        lines
            .iter()
            .enumerate()
            .filter(|(_, line)| line.has_match())
            .map(|(idx, _)| idx.saturating_sub(context)..(idx + context + 1).min(lines.len())),
    );
    windows
        .into_iter()
        .flatten()
        .map(|idx| OutputLine {
            line: number(idx),
            content: lines[idx].map(str::to_owned),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use atuin_common::db::sqlite::fts::TextHighlighter;
    use rstest::rstest;

    use super::*;

    fn body(text: &str, needle: &str) -> HighlightedString {
        let highlighter = TextHighlighter::with_markers(['«', '»']).expect("distinct markers");
        highlighter.as_highlighted(text.replace(needle, &format!("«{needle}»")))
    }

    fn numbered(lines: &[OutputLine]) -> Vec<(i64, String)> {
        lines.iter().map(|l| (l.line, l.content.display_plain().to_string())).collect()
    }

    #[rstest]
    #[case::context_around_a_middle_line("a\nb\nerror\nc\nd", vec![(1, "b"), (2, "error"), (3, "c")])]
    #[case::no_context_before_the_first_line("error\nb\nc", vec![(0, "error"), (1, "b")])]
    #[case::adjacent_windows_merge(
        "error\nb\nerror\nd\ne",
        vec![(0, "error"), (1, "b"), (2, "error"), (3, "d")]
    )]
    #[case::distant_windows_leave_a_gap(
        "error\nb\nc\nd\nerror\nf",
        vec![(0, "error"), (1, "b"), (3, "d"), (4, "error"), (5, "f")]
    )]
    #[case::two_hits_on_one_line_show_it_once("x\nerror error\ny", vec![(0, "x"), (1, "error error"), (2, "y")])]
    #[case::no_hits("a\nb", vec![])]
    fn windows_the_lines_around_each_hit(#[case] text: &str, #[case] expected: Vec<(i64, &str)>) {
        let lines = snippet(&body(text, "error"), None, 1);
        let expected: Vec<(i64, String)> =
            expected.into_iter().map(|(n, s)| (n, s.to_owned())).collect();
        assert_eq!(numbered(&lines), expected);
    }

    #[rstest]
    fn kept_tail_lines_are_numbered_from_the_end() {
        // Five lines; the last two are the kept tail of a truncated output.
        let lines = snippet(&body("a\nerror\nc\nd\nerror", "error"), Some(3), 0);
        assert_eq!(numbered(&lines), vec![(1, "error".to_owned()), (-1, "error".to_owned())]);
    }

    #[rstest]
    fn each_line_keeps_its_own_highlight() {
        let lines = snippet(&body("x\nan error here\ny", "error"), None, 0);
        assert_eq!(lines.len(), 1);
        let ranges: Vec<_> = lines[0].content.to_plain().ranges;
        assert_eq!(ranges, vec![3..8]);
    }
}
