//! The marked block that setup keeps in a shell file that the user also writes, such as
//! `~/.profile`: the comment [`MARKER`] and one line that calls `atuin`.
//!
//! The functions work on bytes, because the user's file may be in any encoding. The markers and
//! the lines that setup matches are ASCII.

use std::ops::Range;

use super::exec::{ExecPlacement, exec_without_grammar, statement_before_exec};
use super::syntax::{LOGIN_SUBCOMMAND, MARKER};

/// The syntax of a file that holds a block, which decides where the block goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Syntax {
    /// `sh` and the shells that read its syntax, such as Bash and Zsh.
    Sh,
    Nu,
}

impl Syntax {
    /// Where setup's block goes in `text`, a file of this syntax.
    fn exec_placement(self, text: &[u8]) -> ExecPlacement {
        match self {
            Self::Sh => statement_before_exec(text),
            Self::Nu => exec_without_grammar(text),
        }
    }
}

/// A file's content with setup's block, from [`with_line`].
pub struct Placed {
    pub text: Vec<u8>,
    pub placement: Placement,
}

/// Where [`with_line`] put the block, relative to an `exec` that replaces the shell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    /// The block runs before the first `exec` of the file that replaces the shell, or the file
    /// doesn't replace the shell.
    BeforeAnyExec,
    /// The block comes after an `exec` that replaces the shell, because setup couldn't tell where
    /// the statement of the `exec` starts.
    ///
    /// The sessions that the `exec` starts don't run the block.
    AfterExec,
}

/// `existing` with one block that holds `line`, in place of setup's earlier blocks.
///
/// The block goes at the end, after a blank line, so that it runs with what the rest of the file
/// sets. In a file of `sh` whose statement replaces the shell with `exec`, such as the
/// `exec sway` of a console login, the block goes before that statement instead, followed by a
/// blank line.
///
/// When setup can't tell where that statement starts, the block goes at the end, and a block
/// that holds `line` stays where the file has it, because the user may have moved the block
/// above the `exec`. Setup can't tell that for any `exec` of a file of Nushell.
pub fn with_line(existing: &[u8], line: &str, syntax: Syntax) -> Placed {
    let mut updated = without_line(existing);
    let before_exec = syntax.exec_placement(&updated);
    let block = MARKER.bytes().chain(*b"\n").chain(line.bytes()).chain(*b"\n");
    let placement = match before_exec {
        ExecPlacement::Before(at) => {
            updated.splice(at..at, block.chain(*b"\n"));
            return Placed {
                text: updated,
                placement: Placement::BeforeAnyExec,
            };
        }
        ExecPlacement::Unknown { .. } => {
            if let Some(placement) = current_block_placement(existing, line, syntax) {
                return Placed {
                    text: existing.to_vec(),
                    placement,
                };
            }
            Placement::AfterExec
        }
        ExecPlacement::NoExec => Placement::BeforeAnyExec,
    };
    match updated.last() {
        None => {}
        Some(b'\n') => updated.push(b'\n'),
        Some(_) => updated.extend_from_slice(b"\n\n"),
    }
    updated.extend(block);
    Placed {
        text: updated,
        placement,
    }
}

/// Where the block of `text` is, relative to the first `exec` of `text` that replaces the shell,
/// when `text`, a file of `syntax`, holds only one block of setup's, and that block holds `line`.
fn current_block_placement(text: &[u8], line: &str, syntax: Syntax) -> Option<Placement> {
    let spans = spans(text);
    let [span] = spans.as_slice() else {
        return None;
    };
    let mut block = lines(&text[span.range.start..span.range.end]);
    let is_current =
        block.next().is_some_and(|marker| marker.trim_ascii_end() == MARKER.as_bytes())
            && block.next().is_some_and(|call| call.trim_ascii_end() == line.as_bytes());
    if !is_current {
        return None;
    }
    Some(match syntax.exec_placement(text) {
        ExecPlacement::Unknown { exec_line } if span.range.start < exec_line => {
            Placement::BeforeAnyExec
        }
        _ => Placement::AfterExec,
    })
}

/// `existing` without setup's blocks, as it was before setup added them.
///
/// The blank line that setup added next to a block goes with the block.
pub fn without_line(existing: &[u8]) -> Vec<u8> {
    let spans = spans(existing);
    let mut updated = Vec::with_capacity(existing.len());
    let mut kept_from = 0;
    for Span {
        range,
        added_blank_line,
    } in spans
    {
        updated.extend_from_slice(&existing[kept_from..range.start]);
        kept_from = range.end;
        if !added_blank_line {
            continue;
        }
        let after = &existing[range.end..];
        if after.starts_with(b"\n") || after.starts_with(b"\r\n") {
            // The blank line between a block before an `exec` and the statement.
            kept_from += if after[0] == b'\r' {
                2
            } else {
                1
            };
        } else if range.end == existing.len() && (updated.ends_with(b"\n\n") || updated == b"\n") {
            // The blank line before a block at the end.
            updated.pop();
        }
    }
    updated.extend_from_slice(&existing[kept_from..]);
    updated
}

/// What setup wrote in a file, and that removal takes out.
struct Span {
    range: Range<usize>,
    /// Whether setup added a blank line next to the span, which goes with the span.
    added_blank_line: bool,
}

/// The lines of `text`, each with its newline.
fn lines(text: &[u8]) -> impl Iterator<Item = &[u8]> {
    text.split_inclusive(|&byte| byte == b'\n')
}

/// Setup's blocks in `text`. Each holds a marker's line and, when it is setup's own, the line
/// after the marker. A line of the user's after a marker stays outside, and so does the blank
/// line next to a marker without a call, which the user may have written.
fn spans(text: &[u8]) -> Vec<Span> {
    let mut spans = Vec::new();
    let mut lines = lines(text).peekable();
    let mut offset = 0;
    while let Some(line) = lines.next() {
        let start = offset;
        offset += line.len();
        if !is_marker(line) {
            continue;
        }
        let with_call = lines.peek().is_some_and(|next| is_setups_call_line(next));
        if with_call {
            offset += lines.next().map_or(0, <[u8]>::len);
        }
        spans.push(Span {
            range: start..offset,
            added_blank_line: with_call,
        });
    }
    spans
}

fn is_marker(line: &[u8]) -> bool {
    line.trim_ascii_end() == MARKER.as_bytes()
}

/// Whether `line`, which follows a marker, is a call that setup wrote. A line that the user
/// commented out is the user's.
fn is_setups_call_line(line: &[u8]) -> bool {
    let line = line.trim_ascii();
    if line.starts_with(b"#") {
        return false;
    }
    let call = LOGIN_SUBCOMMAND.as_bytes();
    line.windows(call.len()).any(|window| window == call)
}

#[cfg(test)]
mod tests;
