//! The changes a tool call made to files, as its harness recorded them: Claude Code's
//! `structuredPatch`, pi's `details.patch`, opencode's `filediff` and `files`, Codex's `FileChange`
//! items. Each is read into the same shape, [`Patch`], a unified diff's hunks per file, so a
//! transcript written back out can give its harness the diff it shows an edit with, and a reader
//! can show one whatever harness made it.

use std::fmt::Write as _;

use serde::{Deserialize, Serialize};

use crate::harnesstools::session::ToolCallId;

/// What a tool call changed: the files, in the order its harness listed them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Patch {
    /// The call that made the changes.
    pub call: ToolCallId,
    pub files: Vec<FilePatch>,
}

/// One file's changes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FilePatch {
    /// The file, as the harness named it (absolute or relative to the session's directory).
    pub path: String,
    pub change: Change,
    /// Where an update moved the file to, if it did.
    pub moved_to: Option<String>,
    /// The changed lines with their context. An added file's lines are all `+`, a deleted one's
    /// all `-`, where the harness recorded them; a harness that keeps a new file's content only in
    /// the call's input (Claude Code's `Write`) records no hunks for it.
    pub hunks: Vec<Hunk>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Change {
    Add,
    Update,
    Delete,
}

/// A unified diff hunk: `@@ -old_start,old_lines +new_start,new_lines @@` and its lines, each
/// starting with ` ` (context), `-`, `+`, or `\` (`\ No newline at end of file`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hunk {
    pub old_start: u64,
    pub old_lines: u64,
    pub new_start: u64,
    pub new_lines: u64,
    pub lines: Vec<String>,
}

impl Patch {
    /// The files changed, each with its lines added and removed: `src/lib.rs +2 -1, a.rs +3`.
    #[must_use]
    pub fn summary(&self) -> String {
        let files: Vec<String> = self
            .files
            .iter()
            .map(|file| {
                let mut line = file.path.clone();
                if let Some(to) = &file.moved_to {
                    let _ = write!(line, " → {to}");
                }
                match (file.change, file.additions(), file.deletions()) {
                    (Change::Add, 0, _) => line.push_str(" (added)"),
                    (Change::Delete, _, 0) => line.push_str(" (deleted)"),
                    (_, added, removed) => {
                        if added > 0 {
                            let _ = write!(line, " +{added}");
                        }
                        if removed > 0 {
                            let _ = write!(line, " -{removed}");
                        }
                    }
                }
                line
            })
            .collect();
        files.join(", ")
    }

    /// Every file's changes as one unified diff.
    #[must_use]
    pub fn unified(&self) -> String {
        self.files.iter().map(FilePatch::unified).collect()
    }
}

impl Change {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Add => "add",
            Self::Update => "update",
            Self::Delete => "delete",
        }
    }
}

impl FilePatch {
    /// A file added with `content`, as one hunk of `+` lines.
    #[must_use]
    pub fn added(path: String, content: &str) -> Self {
        Self {
            path,
            change: Change::Add,
            moved_to: None,
            hunks: Hunk::whole(content, '+').into_iter().collect(),
        }
    }

    /// A file deleted with `content`, as one hunk of `-` lines.
    #[must_use]
    pub fn deleted(path: String, content: &str) -> Self {
        Self {
            path,
            change: Change::Delete,
            moved_to: None,
            hunks: Hunk::whole(content, '-').into_iter().collect(),
        }
    }

    /// Lines added.
    #[must_use]
    pub fn additions(&self) -> usize {
        self.count('+')
    }

    /// Lines removed.
    #[must_use]
    pub fn deletions(&self) -> usize {
        self.count('-')
    }

    fn count(&self, prefix: char) -> usize {
        self.hunks.iter().flat_map(|h| &h.lines).filter(|l| l.starts_with(prefix)).count()
    }

    /// The file's content as one side of its hunks gives it: an added file's (`+`), or a deleted
    /// one's (`-`), lines joined, each ended with a newline unless a `\` line says otherwise.
    #[must_use]
    pub fn side(&self, prefix: char) -> String {
        let mut out = String::new();
        let lines = self.hunks.iter().flat_map(|h| &h.lines);
        let mut last_kept = false;
        for line in lines {
            if line.starts_with('\\') {
                if last_kept && out.ends_with('\n') {
                    out.pop();
                }
                continue;
            }
            last_kept = line.starts_with(prefix) || line.starts_with(' ');
            if last_kept {
                out.push_str(&line[1..]);
                out.push('\n');
            }
        }
        out
    }

    /// The file's hunks as a unified diff, after `---` and `+++` headers naming it.
    #[must_use]
    pub fn unified(&self) -> String {
        let old = if self.change == Change::Add {
            "/dev/null"
        } else {
            &self.path
        };
        let new = match self.change {
            Change::Delete => "/dev/null",
            _ => self.moved_to.as_deref().unwrap_or(&self.path),
        };
        let mut out = format!("--- {old}\n+++ {new}\n");
        out.push_str(&self.hunks_text());
        out
    }

    /// The file's hunks as a unified diff without file headers, as Codex records an update.
    #[must_use]
    pub fn hunks_text(&self) -> String {
        let mut out = String::new();
        for hunk in &self.hunks {
            hunk.write(&mut out);
        }
        out
    }
}

impl Hunk {
    /// All of `content` as one hunk of `prefix` lines: added (`+`) or deleted (`-`). `None` for
    /// empty content.
    fn whole(content: &str, prefix: char) -> Option<Self> {
        if content.is_empty() {
            return None;
        }
        // Split on newlines only: a CRLF file's lines keep their carriage returns.
        let body = content.strip_suffix('\n').unwrap_or(content);
        let mut lines: Vec<String> = body.split('\n').map(|l| format!("{prefix}{l}")).collect();
        let n = lines.len() as u64;
        if !content.ends_with('\n') {
            lines.push("\\ No newline at end of file".to_owned());
        }
        let (old, new) = if prefix == '+' {
            ((0, 0), (1, n))
        } else {
            ((1, n), (0, 0))
        };
        Some(Self {
            old_start: old.0,
            old_lines: old.1,
            new_start: new.0,
            new_lines: new.1,
            lines,
        })
    }

    fn write(&self, out: &mut String) {
        let _ = writeln!(
            out,
            "@@ -{},{} +{},{} @@",
            self.old_start, self.old_lines, self.new_start, self.new_lines
        );
        for line in &self.lines {
            out.push_str(line);
            out.push('\n');
        }
    }

    /// The hunks of a unified diff of one file, any file headers (`---`, `+++`, `Index:`, `===`)
    /// skipped. Lines outside a hunk, and a hunk whose header doesn't parse, are skipped too.
    #[must_use]
    pub fn parse(diff: &str) -> Vec<Self> {
        parse_files(diff).into_iter().flat_map(|file| file.hunks).collect()
    }
}

impl FilePatch {
    /// The files of a unified diff its headers name (see [`parse_files`]).
    #[must_use]
    pub fn from_diff(diff: &str) -> Vec<Self> {
        parse_files(diff)
            .into_iter()
            .filter_map(|file| {
                Some(Self {
                    path: file.path?,
                    change: file.change,
                    moved_to: None,
                    hunks: file.hunks,
                })
            })
            .collect()
    }
}

/// A file of a unified diff, as [`parse_files`] reads it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffFile {
    /// The path its `+++` header names, its `---` header's for a deleted file; `None` for hunks
    /// before any header.
    pub path: Option<String>,
    /// Added when its `---` header is `/dev/null`, deleted when its `+++` header is.
    pub change: Change,
    pub hunks: Vec<Hunk>,
}

/// The files of a unified diff, each with its hunks. A line keeps a carriage return it ends with
/// (a CRLF file's), which is part of the line, not of the diff.
#[must_use]
pub fn parse_files(diff: &str) -> Vec<DiffFile> {
    let mut files: Vec<DiffFile> = Vec::new();
    let mut old_path: Option<String> = None;
    // Lines of the current hunk still expected on each side; a hunk's own `---` or `+++` line
    // (a removed `-- x` line, an added `++ x`) is a line of it, never a header.
    let mut left = (0u64, 0u64);
    // Whether the file's diff is git's (`diff --git a/x b/x`), whose paths carry `a/` and `b/`.
    let mut git = false;
    let lines = diff.strip_suffix('\n').unwrap_or(diff).split('\n');
    for line in lines.filter(|_| !diff.is_empty()) {
        if left != (0, 0) {
            let kind = line.chars().next();
            if let Some(hunk) = files.last_mut().and_then(|f| f.hunks.last_mut())
                && matches!(kind, None | Some(' ' | '-' | '+' | '\\'))
            {
                match kind {
                    Some('-') => left.0 = left.0.saturating_sub(1),
                    Some('+') => left.1 = left.1.saturating_sub(1),
                    Some('\\') => {}
                    _ => left = (left.0.saturating_sub(1), left.1.saturating_sub(1)),
                }
                // An empty context line some writers leave without its space.
                hunk.lines.push(if line.is_empty() {
                    " ".to_owned()
                } else {
                    line.to_owned()
                });
                continue;
            }
            // No hunk line starts so: the hunk was shorter than its header said, and this line
            // is what else it is (the next hunk's header, the next file's).
            left = (0, 0);
        }
        // A `\` line after a hunk's last line belongs to it.
        if line.starts_with('\\')
            && let Some(hunk) = files.last_mut().and_then(|f| f.hunks.last_mut())
        {
            hunk.lines.push(line.to_owned());
            continue;
        }
        if line.starts_with("diff --git ") {
            git = true;
            continue;
        }
        if let Some(path) = line.strip_prefix("--- ") {
            old_path = Some(header_path(path));
            continue;
        }
        if let Some(path) = line.strip_prefix("+++ ") {
            let new = header_path(path);
            let old = old_path.take();
            let (path, change) = match (old, new.as_str()) {
                // A deleted file: named by its `---` header.
                (old, "/dev/null") => (old, Change::Delete),
                (Some(old), new) if old == "/dev/null" => {
                    (Some(git_path(git, &old, new).to_owned()), Change::Add)
                }
                (Some(old), new) => (Some(git_path(git, &old, new).to_owned()), Change::Update),
                (None, _) => (Some(new), Change::Update),
            };
            files.push(DiffFile {
                path,
                change,
                hunks: Vec::new(),
            });
            continue;
        }
        if let Some(hunk) = hunk_header(line) {
            left = (hunk.old_lines, hunk.new_lines);
            if files.is_empty() {
                files.push(DiffFile {
                    path: None,
                    change: Change::Update,
                    hunks: Vec::new(),
                });
            }
            files.last_mut().expect("a file").hunks.push(hunk);
        }
    }
    files
}

/// A `---`/`+++` header's path, without a timestamp after a tab.
fn header_path(header: &str) -> String {
    header.split('\t').next().unwrap_or(header).trim_end().to_owned()
}

/// The `+++` header's path `new` without git's `b/` prefix: when the `---` header's `old` is the
/// same path with its `a/`, or the diff is `git`'s (an added file's `---` is `/dev/null`, which
/// says nothing of prefixes). In any other diff, a path in a directory named `b` keeps it.
fn git_path<'p>(git: bool, old: &str, new: &'p str) -> &'p str {
    match (old.strip_prefix("a/"), new.strip_prefix("b/")) {
        (Some(old), Some(path)) if old == path => path,
        (None, Some(path)) if git && old == "/dev/null" => path,
        _ => new,
    }
}

/// A hunk, without lines, from its `@@ -a,b +c,d @@` header (a count left out is 1).
fn hunk_header(line: &str) -> Option<Hunk> {
    let rest = line.strip_prefix("@@ -")?;
    let (ranges, _) = rest.split_once(" @@")?;
    let (old, new) = ranges.split_once(" +")?;
    let range = |r: &str| -> Option<(u64, u64)> {
        match r.split_once(',') {
            Some((start, len)) => Some((start.parse().ok()?, len.parse().ok()?)),
            None => Some((r.parse().ok()?, 1)),
        }
    };
    let (old_start, old_lines) = range(old)?;
    let (new_start, new_lines) = range(new)?;
    Some(Hunk {
        old_start,
        old_lines,
        new_start,
        new_lines,
        lines: Vec::new(),
    })
}

/// `hunks` as pi's `edit` tool writes its `details.diff` (`generateDiffString`): each line its
/// prefix, its line number padded to the widest, a space and its text (`+` lines numbered in the
/// new file, the rest in the old), with `...` for the lines skipped before and between hunks. Also
/// the first changed line, numbered in the new file (`firstChangedLine`).
#[must_use]
pub fn pi_diff(hunks: &[Hunk]) -> (String, Option<u64>) {
    let width = hunks
        .iter()
        // The last line each side shows.
        .map(|h| (h.old_start + h.old_lines).max(h.new_start + h.new_lines).saturating_sub(1))
        .max()
        .unwrap_or(0)
        .to_string()
        .len();
    let mut out = Vec::new();
    let mut first = None;
    for (i, hunk) in hunks.iter().enumerate() {
        if i > 0 || hunk.old_start > 1 {
            out.push(format!(" {:>width$} ...", ""));
        }
        let (mut old, mut new) = (hunk.old_start, hunk.new_start);
        for line in &hunk.lines {
            let (prefix, text) = line.split_at(line.chars().next().map_or(0, char::len_utf8));
            let number = match prefix {
                "+" => {
                    first.get_or_insert(new);
                    new += 1;
                    new - 1
                }
                "-" => {
                    first.get_or_insert(new);
                    old += 1;
                    old - 1
                }
                " " => {
                    old += 1;
                    new += 1;
                    old - 1
                }
                _ => continue,
            };
            out.push(format!("{prefix}{number:>width$} {text}"));
        }
    }
    (out.join("\n"), first)
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use proptest::prelude::*;
    use rstest::rstest;

    use super::*;

    fn hunk(old: (u64, u64), new: (u64, u64), lines: &[&str]) -> Hunk {
        Hunk {
            old_start: old.0,
            old_lines: old.1,
            new_start: new.0,
            new_lines: new.1,
            lines: lines.iter().map(|l| (*l).to_owned()).collect(),
        }
    }

    fn update(hunks: Vec<Hunk>) -> FilePatch {
        FilePatch {
            path: "src/lib.rs".to_owned(),
            change: Change::Update,
            moved_to: None,
            hunks,
        }
    }

    #[rstest]
    // Codex: hunks only.
    #[case::bare("@@ -2,2 +2,2 @@\n bar\n-baz\n+BAZ\n", None)]
    // pi: file headers only.
    #[case::headers("--- a.ts\n+++ a.ts\n@@ -2,2 +2,2 @@\n bar\n-baz\n+BAZ\n", Some("a.ts"))]
    // opencode (`createTwoFilesPatch`): an index line and timestamps after the paths.
    #[case::jsdiff(
        "Index: a.ts\n===================================================================\n--- \
         a.ts\told\n+++ a.ts\tnew\n@@ -2,2 +2,2 @@\n bar\n-baz\n+BAZ\n",
        Some("a.ts")
    )]
    #[case::git("--- a/a.ts\n+++ b/a.ts\n@@ -2,2 +2,2 @@\n bar\n-baz\n+BAZ\n", Some("a.ts"))]
    #[case::directory_named_b(
        "--- b/a.ts\n+++ b/a.ts\n@@ -2,2 +2,2 @@\n bar\n-baz\n+BAZ\n",
        Some("b/a.ts")
    )]
    fn parses_unified_diffs(#[case] diff: &str, #[case] path: Option<&str>) {
        assert_eq!(parse_files(diff), vec![DiffFile {
            path: path.map(str::to_owned),
            change: Change::Update,
            hunks: vec![hunk((2, 2), (2, 2), &[" bar", "-baz", "+BAZ"])],
        }]);
    }

    #[rstest]
    fn parses_lines_that_look_like_headers_inside_a_hunk() {
        let diff = "@@ -1,2 +1,2 @@\n--- old\n+++ new\n-x\n+y\n";
        let hunks = Hunk::parse(diff);
        assert_eq!(hunks, vec![hunk((1, 2), (1, 2), &["--- old", "+++ new", "-x", "+y"])]);
    }

    #[rstest]
    fn parses_several_files() {
        let diff = "--- a\n+++ a\n@@ -1 +1 @@\n-x\n+y\n--- b\n+++ /dev/null\n@@ -1 +0,0 \
                    @@\n-z\n--- /dev/null\n+++ c\n@@ -0,0 +1 @@\n+w\n";
        assert_eq!(parse_files(diff), vec![
            DiffFile {
                path: Some("a".to_owned()),
                change: Change::Update,
                hunks: vec![hunk((1, 1), (1, 1), &["-x", "+y"])],
            },
            DiffFile {
                path: Some("b".to_owned()),
                change: Change::Delete,
                hunks: vec![hunk((1, 1), (0, 0), &["-z"])],
            },
            DiffFile {
                path: Some("c".to_owned()),
                change: Change::Add,
                hunks: vec![hunk((0, 0), (1, 1), &["+w"])],
            },
        ]);
    }

    /// An added file keeps a directory named `b`, unless the diff is git's.
    #[rstest]
    #[case::plain("--- /dev/null\n+++ b/new.rs\n@@ -0,0 +1 @@\n+x\n", "b/new.rs")]
    #[case::git(
        "diff --git a/new.rs b/new.rs\nnew file mode 100644\n--- /dev/null\n+++ b/new.rs\n@@ -0,0 \
         +1 @@\n+x\n",
        "new.rs"
    )]
    fn an_added_files_path(#[case] diff: &str, #[case] path: &str) {
        let files = FilePatch::from_diff(diff);
        assert_eq!(files.len(), 1);
        assert_eq!((files[0].path.as_str(), files[0].change), (path, Change::Add));
        let added = FilePatch::added("b/new.rs".to_owned(), "x\n");
        assert_eq!(FilePatch::from_diff(&added.unified()), vec![added]);
    }

    /// A hunk shorter than its header says ends at the next line no hunk line starts so: the
    /// next hunk's header is still read, and its lines with it.
    #[rstest]
    fn a_short_hunk_does_not_swallow_the_next() {
        let diff = "@@ -1,5 +1,5 @@\n-x\n+y\n@@ -20,1 +20,1 @@\n-a\n+b\n";
        assert_eq!(Hunk::parse(diff), vec![
            hunk((1, 5), (1, 5), &["-x", "+y"]),
            hunk((20, 1), (20, 1), &["-a", "+b"]),
        ]);
    }

    /// A CRLF file's lines keep their carriage returns, through a diff and through its content.
    #[rstest]
    fn carriage_returns_are_part_of_the_line() {
        let diff = "--- a\r\n+++ a\r\n@@ -1 +1 @@\r\n-x\r\n+y\r\n";
        assert_eq!(FilePatch::from_diff(diff), vec![FilePatch {
            path: "a".to_owned(),
            change: Change::Update,
            moved_to: None,
            hunks: vec![hunk((1, 1), (1, 1), &["-x\r", "+y\r"])],
        }]);
        let content = "a\r\nb\r\n";
        assert_eq!(FilePatch::added("f".to_owned(), content).side('+'), content);
    }

    #[rstest]
    #[case::mid_hunk(
        "@@ -1 +1 @@\n-x\n\\ No newline at end of file\n+y\n",
        &["-x", "\\ No newline at end of file", "+y"]
    )]
    #[case::last_line(
        "@@ -1 +1 @@\n-x\n+y\n\\ No newline at end of file\n",
        &["-x", "+y", "\\ No newline at end of file"]
    )]
    fn keeps_a_missing_newline_marker(#[case] diff: &str, #[case] lines: &[&str]) {
        assert_eq!(Hunk::parse(diff), vec![hunk((1, 1), (1, 1), lines)]);
    }

    #[rstest]
    #[case::trailing_newline("a\nb\n")]
    #[case::no_trailing_newline("a\nb")]
    fn added_and_deleted_files_give_their_content_back(#[case] content: &str) {
        assert_eq!(FilePatch::added("f".to_owned(), content).side('+'), content);
        assert_eq!(FilePatch::deleted("f".to_owned(), content).side('-'), content);
    }

    #[rstest]
    fn counts_additions_and_deletions() {
        let file = update(vec![hunk((1, 3), (1, 4), &[" a", "-b", "+c", "+d", " e"])]);
        assert_eq!((file.additions(), file.deletions()), (2, 1));
    }

    #[rstest]
    fn summarises_the_files_changed() {
        let patch = Patch {
            call: ToolCallId::from("c".to_owned()),
            files: vec![
                update(vec![hunk((1, 3), (1, 4), &[" a", "-b", "+c", "+d", " e"])]),
                FilePatch {
                    moved_to: Some("new.rs".to_owned()),
                    ..update(Vec::new())
                },
                FilePatch::added("empty".to_owned(), ""),
                FilePatch::deleted("gone".to_owned(), "x\n"),
            ],
        };
        assert_eq!(
            patch.summary(),
            "src/lib.rs +2 -1, src/lib.rs → new.rs, empty (added), gone -1"
        );
    }

    #[rstest]
    fn writes_pi_diffs() {
        let hunks = vec![
            hunk((1, 3), (1, 3), &[" one", "-two", "+TWO", " three"]),
            hunk((9, 2), (9, 3), &[" nine", "+nine and a half", " ten"]),
        ];
        let (diff, first) = pi_diff(&hunks);
        assert_eq!(
            diff,
            "  1 one\n- 2 two\n+ 2 TWO\n  3 three\n    ...\n  9 nine\n+10 nine and a half\n 10 ten"
        );
        assert_eq!(first, Some(2));
    }

    fn line() -> impl Strategy<Value = String> {
        // Text a line could hold, header-like starts included.
        prop_oneof![Just("--- x".to_owned()), Just("+++ y".to_owned()), "[ -~]{0,12}"]
    }

    fn file() -> impl Strategy<Value = FilePatch> {
        prop::collection::vec(
            (
                prop::collection::vec(line(), 0..3),
                prop::collection::vec(line(), 0..3),
                prop::collection::vec(line(), 0..3),
            ),
            0..4,
        )
        .prop_map(|parts| {
            let mut at = 1;
            let hunks = parts
                .into_iter()
                .map(|(context, removed, added)| {
                    let mut lines: Vec<String> = context.iter().map(|l| format!(" {l}")).collect();
                    lines.extend(removed.iter().map(|l| format!("-{l}")));
                    lines.extend(added.iter().map(|l| format!("+{l}")));
                    let old = (context.len() + removed.len()) as u64;
                    let new = (context.len() + added.len()) as u64;
                    let hunk = hunk((at, old), (at, new), &[]);
                    at += old.max(new) + 10;
                    Hunk { lines, ..hunk }
                })
                .collect();
            update(hunks)
        })
    }

    proptest! {
        #[test]
        fn unified_diffs_round_trip(file in file()) {
            prop_assert_eq!(FilePatch::from_diff(&file.unified()), vec![file.clone()]);
            prop_assert_eq!(Hunk::parse(&file.hunks_text()), file.hunks);
        }
    }
}
