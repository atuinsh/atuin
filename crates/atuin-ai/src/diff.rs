//! Structured diff computation for edit previews.
//!
//! Computes a line-level diff between old and new file content using
//! imara-diff's Histogram algorithm, producing structured hunks with
//! typed lines (Context, Added, Removed) suitable for TUI rendering.

use imara_diff::{Algorithm, Diff, InternedInput};

/// Number of context lines to show around each change.
const CONTEXT_LINES: u32 = 3;

/// A structured diff preview for a file edit, ready for rendering.
#[derive(Debug, Clone)]
pub(crate) struct EditPreview {
    pub hunks: Vec<DiffHunk>,
}

/// A contiguous group of diff lines (context + changes).
#[derive(Debug, Clone)]
pub(crate) struct DiffHunk {
    /// 1-indexed line number of the first line in this hunk (in the original file).
    pub before_start: u32,
    /// 1-indexed line number of the first line in this hunk (in the new file).
    pub after_start: u32,
    pub lines: Vec<DiffLine>,
}

/// A single line in a diff hunk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DiffLine {
    /// Unchanged line (shown for context).
    Context(String),
    /// Line added in the new version.
    Added(String),
    /// Line removed from the old version.
    Removed(String),
}

impl EditPreview {
    /// Compute a structured diff between old and new file content.
    ///
    /// Uses the Histogram algorithm with line-level granularity and
    /// indentation-aware postprocessing for readable output.
    pub fn compute(old: &str, new: &str) -> Self {
        let input = InternedInput::new(old, new);
        let mut diff = Diff::compute(Algorithm::Histogram, &input);
        diff.postprocess_lines(&input);

        let raw_hunks: Vec<_> = diff.hunks().collect();
        if raw_hunks.is_empty() {
            return EditPreview { hunks: Vec::new() };
        }

        // Merge hunks that are within 2*CONTEXT_LINES of each other
        // (same logic as unified diff format).
        let mut merged_groups: Vec<Vec<&imara_diff::Hunk>> = Vec::new();
        let mut current_group: Vec<&imara_diff::Hunk> = vec![&raw_hunks[0]];

        for hunk in &raw_hunks[1..] {
            let prev = current_group.last().unwrap();
            if hunk.before.start.saturating_sub(prev.before.end) <= 2 * CONTEXT_LINES {
                current_group.push(hunk);
            } else {
                merged_groups.push(current_group);
                current_group = vec![hunk];
            }
        }
        merged_groups.push(current_group);

        // Build structured hunks from merged groups
        let hunks = merged_groups
            .into_iter()
            .map(|group| build_hunk(&group, &input))
            .collect();

        EditPreview { hunks }
    }

    /// The highest line number (from either file) that will be displayed.
    /// Used to calculate gutter width.
    pub fn max_line_number(&self) -> u32 {
        self.hunks
            .iter()
            .map(|h| {
                let mut before_pos = h.before_start;
                let mut after_pos = h.after_start;
                for line in &h.lines {
                    match line {
                        DiffLine::Context(_) => {
                            before_pos += 1;
                            after_pos += 1;
                        }
                        DiffLine::Removed(_) => before_pos += 1,
                        DiffLine::Added(_) => after_pos += 1,
                    }
                }
                before_pos.max(after_pos).saturating_sub(1)
            })
            .max()
            .unwrap_or(0)
    }
}

/// Maximum lines to show in a write preview.
const WRITE_PREVIEW_LINES: usize = 10;

/// A content preview for a write_file operation.
///
/// Shows the first N lines of the written content plus a count of
/// remaining lines if truncated.
#[derive(Debug, Clone)]
pub(crate) struct WritePreview {
    /// First lines of content (up to WRITE_PREVIEW_LINES).
    pub lines: Vec<String>,
    /// Total number of lines in the written file.
    pub total_lines: usize,
}

impl WritePreview {
    /// Create a preview from file content.
    pub fn from_content(content: &str) -> Self {
        let all_lines: Vec<&str> = content.lines().collect();
        let total_lines = all_lines.len();
        let lines = all_lines
            .into_iter()
            .take(WRITE_PREVIEW_LINES)
            .map(String::from)
            .collect();
        WritePreview { lines, total_lines }
    }

    /// Number of lines not shown in the preview.
    pub fn remaining_lines(&self) -> usize {
        self.total_lines.saturating_sub(self.lines.len())
    }
}

/// Build a single DiffHunk from a group of adjacent raw hunks.
fn build_hunk(group: &[&imara_diff::Hunk], input: &InternedInput<&str>) -> DiffHunk {
    let first = group.first().unwrap();
    let last = group.last().unwrap();

    let context_start = first.before.start.saturating_sub(CONTEXT_LINES);
    let context_end = (last.before.end + CONTEXT_LINES).min(input.before.len() as u32);

    // The after-file position of context_start: same offset as before since
    // context before the first change is identical in both files.
    let after_context_start = first.after.start - (first.before.start - context_start);

    let mut lines = Vec::new();
    let mut pos = context_start;

    for hunk in group {
        // Context lines before this hunk
        for i in pos..hunk.before.start {
            lines.push(DiffLine::Context(token_text(input, true, i)));
        }

        // Removed lines
        for i in hunk.before.start..hunk.before.end {
            lines.push(DiffLine::Removed(token_text(input, true, i)));
        }

        // Added lines
        for i in hunk.after.start..hunk.after.end {
            lines.push(DiffLine::Added(token_text(input, false, i)));
        }

        pos = hunk.before.end;
    }

    // Trailing context
    for i in pos..context_end {
        lines.push(DiffLine::Context(token_text(input, true, i)));
    }

    DiffHunk {
        before_start: context_start + 1,      // 1-indexed
        after_start: after_context_start + 1, // 1-indexed
        lines,
    }
}

/// Extract the text content of a token, trimming the trailing newline
/// that imara-diff includes in line-based tokenization.
fn token_text(input: &InternedInput<&str>, is_before: bool, idx: u32) -> String {
    let tokens = if is_before {
        &input.before
    } else {
        &input.after
    };
    let text = input.interner[tokens[idx as usize]];
    text.strip_suffix('\n')
        .unwrap_or(text)
        .strip_suffix('\r')
        .unwrap_or(text.strip_suffix('\n').unwrap_or(text))
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_changes_produces_empty_preview() {
        let preview = EditPreview::compute("hello\nworld\n", "hello\nworld\n");
        assert!(preview.hunks.is_empty());
    }

    #[test]
    fn single_line_replacement() {
        let old = "line1\nline2\nline3\n";
        let new = "line1\nchanged\nline3\n";
        let preview = EditPreview::compute(old, new);

        assert_eq!(preview.hunks.len(), 1);
        let hunk = &preview.hunks[0];

        // Should have: context(line1), removed(line2), added(changed), context(line3)
        assert!(hunk.lines.contains(&DiffLine::Context("line1".into())));
        assert!(hunk.lines.contains(&DiffLine::Removed("line2".into())));
        assert!(hunk.lines.contains(&DiffLine::Added("changed".into())));
        assert!(hunk.lines.contains(&DiffLine::Context("line3".into())));
    }

    #[test]
    fn addition_only() {
        let old = "aaa\nbbb\n";
        let new = "aaa\nnew_line\nbbb\n";
        let preview = EditPreview::compute(old, new);

        assert_eq!(preview.hunks.len(), 1);
        let hunk = &preview.hunks[0];
        assert!(hunk.lines.contains(&DiffLine::Added("new_line".into())));
        // Original lines are context
        assert!(hunk.lines.contains(&DiffLine::Context("aaa".into())));
        assert!(hunk.lines.contains(&DiffLine::Context("bbb".into())));
    }

    #[test]
    fn removal_only() {
        let old = "aaa\nremove_me\nbbb\n";
        let new = "aaa\nbbb\n";
        let preview = EditPreview::compute(old, new);

        assert_eq!(preview.hunks.len(), 1);
        let hunk = &preview.hunks[0];
        assert!(hunk.lines.contains(&DiffLine::Removed("remove_me".into())));
    }

    #[test]
    fn distant_changes_produce_separate_hunks() {
        // Two changes separated by more than 2*CONTEXT_LINES (6) lines
        let old = "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n11\n12\n";
        let new = "1\nX\n3\n4\n5\n6\n7\n8\n9\n10\n11\nY\n";
        let preview = EditPreview::compute(old, new);

        assert_eq!(preview.hunks.len(), 2);
    }

    #[test]
    fn close_changes_merge_into_one_hunk() {
        // Two changes separated by fewer than 2*CONTEXT_LINES lines
        let old = "1\n2\n3\n4\n5\n";
        let new = "X\n2\n3\n4\nY\n";
        let preview = EditPreview::compute(old, new);

        assert_eq!(preview.hunks.len(), 1);
    }

    #[test]
    fn context_is_limited() {
        // With CONTEXT_LINES=3, a change at line 10 shouldn't include line 1
        let mut old_lines: Vec<&str> = (1..=20).map(|_| "unchanged").collect();
        old_lines[9] = "target";
        let old = old_lines.join("\n") + "\n";
        let new = old.replace("target", "replaced");

        let preview = EditPreview::compute(&old, &new);
        assert_eq!(preview.hunks.len(), 1);

        // Should have at most 3 context lines before + 3 after + 1 removed + 1 added = 8 lines
        assert!(preview.hunks[0].lines.len() <= 8);
    }

    #[test]
    fn max_line_number_reflects_file_position() {
        let old = "a\nb\nc\n";
        let new = "a\nX\nc\n";
        let preview = EditPreview::compute(old, new);
        // 3-line file, context + removed lines span positions 1-3
        assert_eq!(preview.max_line_number(), 3);
    }

    #[test]
    fn start_line_is_correct_for_later_changes() {
        // Change at line 10 with 3 context lines → before_start = 7
        let mut lines: Vec<String> = (1..=15).map(|i| format!("line{i}")).collect();
        let old = lines.join("\n") + "\n";
        lines[9] = "CHANGED".to_string();
        let new = lines.join("\n") + "\n";

        let preview = EditPreview::compute(&old, &new);
        assert_eq!(preview.hunks.len(), 1);
        assert_eq!(preview.hunks[0].before_start, 7); // line 10 - 3 context = line 7
        assert_eq!(preview.hunks[0].after_start, 7); // same for a simple replacement
    }

    #[test]
    fn multiline_replacement() {
        let old = "[section]\nkey1 = old1\nkey2 = old2\n[other]\n";
        let new = "[section]\nkey1 = new1\nkey2 = new2\n[other]\n";
        let preview = EditPreview::compute(old, new);

        assert_eq!(preview.hunks.len(), 1);
        let hunk = &preview.hunks[0];
        assert!(
            hunk.lines
                .contains(&DiffLine::Removed("key1 = old1".into()))
        );
        assert!(
            hunk.lines
                .contains(&DiffLine::Removed("key2 = old2".into()))
        );
        assert!(hunk.lines.contains(&DiffLine::Added("key1 = new1".into())));
        assert!(hunk.lines.contains(&DiffLine::Added("key2 = new2".into())));
    }
}
