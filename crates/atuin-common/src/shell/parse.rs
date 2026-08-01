//! Shell command syntax classification and extraction, backed by tree-sitter.
//! Tree-sitter is a private detail of this module; callers see only `Token`,
//! `Command`, and the `ShellParser` trait.

use std::ops::Range;

/// One classified span of a command line. `range` indexes bytes of the input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    pub range: Range<usize>,
    pub kind: TokenKind,
}

/// Neutral highlight categories. Mirrors `atuin_client::theme::Meaning::Syntax*`
/// 1:1, but defined here so `atuin-common` does not depend on the theme.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    Command,
    Flag,
    String,
    Variable,
    Operator,
    Comment,
}

/// One extracted command. Both fields borrow the input; `name` is the leading
/// slice of `full`. No allocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Command<'a> {
    pub name: &'a str,
    pub full: &'a str,
}

/// Classifies one shell dialect's command-line syntax. Impls are stateless ZSTs
/// obtained from [`crate::shell::ShellKind::parser`].
pub trait ShellParser: Send + Sync {
    /// Classify every span of `code`, in paint order (a later span refines an
    /// earlier overlapping one). Empty means "no highlighting".
    ///
    /// Implementations MUST return tokens in non-decreasing byte-start order
    /// (in particular `Command` and `Operator` tokens): [`commands`] relies on
    /// this ordering to compute each command's `full` span.
    fn classify(&self, code: &str) -> Vec<Token>;
}

/// Compile-time proof of object-safety (mirrors the one guarding `Shell`).
const _: fn(&dyn ShellParser) = |_p| {};

/// Word-level parser for shells with no grammar (nu, xonsh, unknown). Emits a
/// `Command` token for each `; | || &&`-delimited segment's first word and an
/// `Operator` token for each separator — enough for [`commands`] to work.
pub struct Fallback;

impl ShellParser for Fallback {
    fn classify(&self, code: &str) -> Vec<Token> {
        let mut out = Vec::new();
        let bytes = code.as_bytes();
        let (mut seg_start, mut i) = (0usize, 0usize);
        while i < bytes.len() {
            let sep = match bytes[i] {
                b';' => Some(1),
                b'|' => Some(if bytes.get(i + 1) == Some(&b'|') { 2 } else { 1 }),
                b'&' if bytes.get(i + 1) == Some(&b'&') => Some(2),
                _ => None,
            };
            if let Some(len) = sep {
                push_first_word(code, seg_start, i, &mut out);
                out.push(Token { range: i..i + len, kind: TokenKind::Operator });
                i += len;
                seg_start = i;
            } else {
                i += 1;
            }
        }
        push_first_word(code, seg_start, bytes.len(), &mut out);
        out
    }
}

/// Emit a `Command` token for the first whitespace-delimited word of `code[start..end]`.
fn push_first_word(code: &str, start: usize, end: usize, out: &mut Vec<Token>) {
    let seg = &code[start..end];
    let lead = seg.len() - seg.trim_start().len();
    let trimmed = seg.trim_start();
    if let Some(word) = trimmed.split_whitespace().next() {
        let name_start = start + lead;
        out.push(Token {
            range: name_start..name_start + word.len(),
            kind: TokenKind::Command,
        });
    }
}

/// Extract every command that will run in `code`, using only `Command` and
/// `Operator` tokens: a `Command` token opens a command; the next `Command` or
/// `Operator` token, or end of input, closes it. `full` runs from the command
/// name to that boundary (trimmed), so plain-word args are covered without tokens
/// of their own; assignments and redirects fall out because the command opens at
/// its name and a redirect operator is a boundary.
///
/// Note the bash/fish asymmetry this produces: fish redirects aren't anonymous
/// operator-char tokens (unlike bash's `>`), so a fish command's `full` includes
/// a trailing redirect (`"ls > out.txt"`) while the posix/bash parser truncates
/// at `>` (`"ls"`); both are fail-safe.
pub fn commands<'a>(parser: &dyn ShellParser, code: &'a str) -> Vec<Command<'a>> {
    let mut cmds = Vec::new();
    let mut open: Option<Range<usize>> = None;
    for tok in parser.classify(code) {
        match tok.kind {
            TokenKind::Command => {
                close(code, open.take(), tok.range.start, &mut cmds);
                open = Some(tok.range);
            }
            TokenKind::Operator => close(code, open.take(), tok.range.start, &mut cmds),
            _ => {}
        }
    }
    close(code, open, code.len(), &mut cmds);
    cmds
}

fn close<'a>(code: &'a str, open: Option<Range<usize>>, end: usize, out: &mut Vec<Command<'a>>) {
    if let Some(name) = open {
        out.push(Command {
            name: &code[name.start..name.end],
            full: code[name.start..end].trim_end(),
        });
    }
}

pub(super) fn classify_with(language: tree_sitter::Language, code: &str) -> Vec<Token> {
    let mut parser = tree_sitter::Parser::new();
    if parser.set_language(&language).is_ok()
        && let Some(tree) = parser.parse(code, None)
    {
        let mut out = Vec::new();
        walk_tokens(tree.root_node(), code.as_bytes(), &mut out);
        out
    } else {
        // Rare: tree-sitter normally returns a tree with ERROR nodes, not None.
        // Preserve the historical word-split fallback.
        Fallback.classify(code)
    }
}

fn walk_tokens(node: tree_sitter::Node, src: &[u8], out: &mut Vec<Token>) {
    let kind = match node.kind() {
        "comment" => Some(TokenKind::Comment),
        "command_name" => Some(TokenKind::Command),
        "string" | "raw_string" | "ansi_c_string" | "heredoc_body"
        | "single_quote_string" | "double_quote_string" => Some(TokenKind::String),
        "simple_expansion" | "expansion" | "variable_assignment" | "variable_expansion" => {
            Some(TokenKind::Variable)
        }
        "word" if src.get(node.start_byte()) == Some(&b'-') => Some(TokenKind::Flag),
        k if !node.is_named()
            && !k.is_empty()
            && k.bytes().all(|b| b"|&;<>(){}$`".contains(&b)) =>
        {
            Some(TokenKind::Operator)
        }
        _ => None,
    };
    if let Some(kind) = kind {
        out.push(Token { range: node.byte_range(), kind });
    }
    // Fish has no `command_name` node; the command's `name` field points at a
    // plain `word`, which the match above never tags. Bash's `command` node
    // also has a `name` field, but it points at a `command_name` node that
    // *is* tagged above, so only synthesize the token when the field's target
    // wouldn't otherwise self-classify (skip for bash, fire for fish).
    if node.kind() == "command"
        && let Some(name) = node.child_by_field_name("name")
        && name.kind() != "command_name"
    {
        out.push(Token { range: name.byte_range(), kind: TokenKind::Command });
    }
    // An expansion is uniformly a variable; don't let its `$`/`{`/`}` children
    // overwrite it as operators.
    if matches!(node.kind(), "simple_expansion" | "expansion" | "variable_expansion") {
        return;
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk_tokens(child, src, out);
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use rstest::rstest;
    use super::{Fallback, Token, TokenKind, commands, ShellParser};

    #[test]
    fn fallback_emits_command_and_operator_tokens() {
        let toks = Fallback.classify("a && b | c");
        assert_eq!(
            toks,
            vec![
                Token { range: 0..1, kind: TokenKind::Command },   // a
                Token { range: 2..4, kind: TokenKind::Operator },  // &&
                Token { range: 5..6, kind: TokenKind::Command },   // b
                Token { range: 7..8, kind: TokenKind::Operator },  // |
                Token { range: 9..10, kind: TokenKind::Command },  // c
            ]
        );
    }

    #[rstest]
    #[case::single("ls -la /tmp", &[("ls", "ls -la /tmp")])]
    #[case::and_or(
        "ls && cat foo || echo fail",
        &[("ls", "ls"), ("cat", "cat foo"), ("echo", "echo fail")]
    )]
    #[case::pipe("cat foo | grep bar", &[("cat", "cat foo"), ("grep", "grep bar")])]
    #[case::empty("", &[])]
    fn commands_fold_over_fallback(#[case] code: &str, #[case] want: &[(&str, &str)]) {
        let got: Vec<(&str, &str)> =
            commands(&Fallback, code).iter().map(|c| (c.name, c.full)).collect();
        assert_eq!(got, want);
    }

    #[test]
    fn command_name_is_a_leading_slice_of_full() {
        for c in commands(&Fallback, "git commit -m hi") {
            assert!(c.full.starts_with(c.name), "{:?} not a prefix of {:?}", c.name, c.full);
        }
    }
}

#[cfg(all(test, feature = "shell-syntax"))]
mod classify_tests {
    use crate::shell::{ShellKind, TokenKind};
    use rstest::rstest;
    use pretty_assertions::assert_eq;

    // One char per byte: c=command f=flag s=string v=variable o=operator #=comment a=base.
    fn render(cmd: &str, shell: ShellKind) -> String {
        let mut out = vec!['a'; cmd.len()];
        for t in shell.parser().classify(cmd) {
            let ch = match t.kind {
                TokenKind::Command => 'c',
                TokenKind::Flag => 'f',
                TokenKind::String => 's',
                TokenKind::Variable => 'v',
                TokenKind::Operator => 'o',
                TokenKind::Comment => '#',
            };
            for slot in &mut out[t.range] {
                *slot = ch;
            }
        }
        out.into_iter().collect()
    }

    #[rstest]
    #[case::simple_command("git commit -m 'hi'", ShellKind::Bash, "cccaaaaaaaaffassss")]
    #[case::pipe("cat foo | grep bar", ShellKind::Bash, "cccaaaaaoaccccaaaa")]
    #[case::env_assignment("FOO=bar make", ShellKind::Bash, "vvvvvvvacccc")]
    #[case::variables("echo $HOME ${USER}x", ShellKind::Bash, "ccccavvvvvavvvvvvva")]
    #[case::comment("ls # list", ShellKind::Bash, "cca######")]
    #[case::fish_set("set -x PATH $PATH", ShellKind::Fish, "cccaffaaaaaavvvvv")]
    #[case::nu_plain("ls -la", ShellKind::Nu, "ccaaaa")]
    fn classify_renders(#[case] cmd: &str, #[case] shell: ShellKind, #[case] expected: &str) {
        assert_eq!(render(cmd, shell), expected);
    }

    #[rstest]
    fn odd_inputs_do_not_panic(
        #[values("echo 'oops", "if (= 1 2) { }", "", "echo héllo")] cmd: &str,
        #[values(ShellKind::Bash, ShellKind::Fish, ShellKind::Nu)] shell: ShellKind,
    ) {
        assert_eq!(shell.parser().classify(cmd).iter().all(|t| t.range.end <= cmd.len()), true);
    }
}
