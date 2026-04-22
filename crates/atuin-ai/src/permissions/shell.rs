/// Extracted command info from a shell command string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ShellCommand {
    /// The command name (first word), e.g. "git"
    pub name: String,
    /// The full invocation including arguments, e.g. "git commit -m msg"
    pub full: String,
}

/// A parsed shell command with all subcommands extracted.
#[derive(Debug)]
pub(crate) struct ParsedShellCommand {
    pub subcommands: Vec<ShellCommand>,
}

/// Supported shell families for parsing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ShellKind {
    /// POSIX sh, bash, zsh — all share similar syntax
    Posix,
    /// fish shell
    Fish,
    /// nushell or unknown — fallback to word-level extraction
    Other,
}

impl ShellKind {
    pub(crate) fn from_shell_name(name: &str) -> Self {
        match name {
            "bash" | "sh" | "zsh" | "dash" | "ksh" => Self::Posix,
            "fish" => Self::Fish,
            _ => Self::Other,
        }
    }
}

/// Parse a shell command string and extract all subcommands.
pub(crate) fn parse_shell_command(code: &str, shell: ShellKind) -> ParsedShellCommand {
    #[cfg(feature = "tree-sitter")]
    match shell {
        ShellKind::Posix => ts::parse_posix(code),
        ShellKind::Fish => ts::parse_fish(code),
        ShellKind::Other => parse_fallback(code),
    }

    #[cfg(not(feature = "tree-sitter"))]
    {
        let _ = shell;
        parse_fallback(code)
    }
}

// ────────────────────────────────────────────────────────────────
// Tree-sitter parsers (POSIX + Fish)
// Disabled on platforms where tree-sitter doesn't cross-compile
// (e.g. Windows); falls back to word-level extraction.
// ────────────────────────────────────────────────────────────────

#[cfg(feature = "tree-sitter")]
mod ts {
    use super::{ParsedShellCommand, ShellCommand, parse_fallback};
    use tree_sitter_lib::{Parser, Tree};

    fn bash_parser() -> Parser {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_bash::LANGUAGE.into())
            .expect("failed to set bash language");
        parser
    }

    pub(super) fn parse_posix(code: &str) -> ParsedShellCommand {
        let mut parser = bash_parser();
        let Some(tree) = parser.parse(code, None) else {
            return parse_fallback(code);
        };

        let mut commands = Vec::new();
        walk_bash_tree(&tree, code.as_bytes(), &mut commands);
        ParsedShellCommand {
            subcommands: commands,
        }
    }

    /// Leaf node kinds that never contain nested commands.
    const BASH_LEAVES: &[&str] = &[
        "command_name",
        "word",
        "number",
        "simple_expansion",
        "expansion",
        "arithmetic_expansion",
        "ansi_c_string",
        "special_variable_name",
        "variable_name",
        "file_descriptor",
        "heredoc_body",
        "heredoc_start",
        "regex",
        "heredoc_redirect",
    ];

    fn walk_bash_tree(tree: &Tree, source: &[u8], commands: &mut Vec<ShellCommand>) {
        walk_bash_node(tree.root_node(), source, commands);
    }

    fn walk_bash_node(
        node: tree_sitter_lib::Node,
        source: &[u8],
        commands: &mut Vec<ShellCommand>,
    ) {
        match node.kind() {
            "command" => {
                if let Some(cmd) = extract_bash_command(node, source) {
                    commands.push(cmd);
                }
                // Descend into all non-leaf children to find nested commands
                // (e.g. command_substitution inside a string inside a command)
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    if !BASH_LEAVES.contains(&child.kind()) {
                        walk_bash_node(child, source, commands);
                    }
                }
            }
            // Other nodes: descend into all children
            _ => {
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    walk_bash_node(child, source, commands);
                }
            }
        }
    }

    /// Extract the full command string and name from a bash `command` node.
    fn extract_bash_command(node: tree_sitter_lib::Node, source: &[u8]) -> Option<ShellCommand> {
        // A `command` node has children like:
        //   variable_assignment* command_name argument* redirect*
        // We want the command_name and all arguments (skipping assignments and redirects).
        let mut name = None;
        let mut name_start = None;
        let mut arg_end = None;

        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            match child.kind() {
                "command_name" => {
                    name = child.utf8_text(source).ok().map(|s| s.to_string());
                    name_start = Some(child.start_byte());
                }
                "word"
                | "string"
                | "raw_string"
                | "concatenation"
                | "number"
                | "simple_expansion"
                | "expansion"
                | "arithmetic_expansion"
                | "ansi_c_string"
                | "process_substitution" => {
                    arg_end = Some(child.end_byte());
                }
                _ => {}
            }
        }

        let name = name?;
        let full = if let (Some(start), Some(end)) = (name_start, arg_end) {
            std::str::from_utf8(&source[start..end]).ok()?.to_string()
        } else {
            name.clone()
        };

        Some(ShellCommand { name, full })
    }

    // ────────────────────────────────────────────────────────────────
    // Fish parser
    // ────────────────────────────────────────────────────────────────

    fn fish_parser() -> Parser {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_fish::language())
            .expect("failed to set fish language");
        parser
    }

    pub(super) fn parse_fish(code: &str) -> ParsedShellCommand {
        let mut parser = fish_parser();
        let Some(tree) = parser.parse(code, None) else {
            return parse_fallback(code);
        };

        let mut commands = Vec::new();
        walk_fish_tree(&tree, code.as_bytes(), &mut commands);
        ParsedShellCommand {
            subcommands: commands,
        }
    }

    const FISH_COMPOUND: &[&str] = &[
        "conditional_execution",
        "pipe",
        "job",
        "command_substitution",
        "block",
        "for_statement",
        "while_statement",
        "if_statement",
        "switch_statement",
        "function_definition",
        "begin_statement",
        "redirected_statement",
    ];

    fn walk_fish_tree(tree: &Tree, source: &[u8], commands: &mut Vec<ShellCommand>) {
        walk_fish_node(tree.root_node(), source, commands);
    }

    fn walk_fish_node(
        node: tree_sitter_lib::Node,
        source: &[u8],
        commands: &mut Vec<ShellCommand>,
    ) {
        match node.kind() {
            "command" => {
                if let Some(cmd) = extract_fish_command(node, source) {
                    commands.push(cmd);
                }
                // Still descend into compound children (e.g. command_substitution inside a command)
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    if FISH_COMPOUND.contains(&child.kind()) {
                        walk_fish_node(child, source, commands);
                    }
                }
            }
            // Other nodes: descend into all children
            _ => {
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    walk_fish_node(child, source, commands);
                }
            }
        }
    }

    fn extract_fish_command(node: tree_sitter_lib::Node, source: &[u8]) -> Option<ShellCommand> {
        // In fish, a `command` node has:
        //   name (command_name or word) followed by arguments (word, string, etc.)
        let mut name = None;

        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            match child.kind() {
                "command_name" | "word" => {
                    let text = child.utf8_text(source).ok()?.to_string();
                    if name.is_none() {
                        name = Some(text);
                    }
                }
                "string"
                | "concatenation"
                | "command_substitution"
                | "escape_sequence"
                | "double_quote_string"
                | "single_quote_string" => {}
                _ => {}
            }
        }

        let name = name?;
        // Get the full text of the command node
        let full = node.utf8_text(source).ok()?.trim().to_string();

        Some(ShellCommand { name, full })
    }
} // mod ts

// ────────────────────────────────────────────────────────────────
// Fallback (word-level extraction for nushell / unknown shells)
// ────────────────────────────────────────────────────────────────

fn parse_fallback(code: &str) -> ParsedShellCommand {
    // Simple heuristic: split by &&, ||, ;, | and take the first word of each segment.
    // This is intentionally simple — for unknown shells we can't do better.
    let mut commands = Vec::new();
    let mut segment = String::new();
    let mut chars = code.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            ';' => {
                push_segment(&mut segment, &mut commands);
            }
            '|' => {
                if chars.peek() == Some(&'|') {
                    chars.next();
                }
                push_segment(&mut segment, &mut commands);
            }
            '&' if chars.peek() == Some(&'&') => {
                chars.next();
                push_segment(&mut segment, &mut commands);
            }
            _ => segment.push(c),
        }
    }
    push_segment(&mut segment, &mut commands);

    ParsedShellCommand {
        subcommands: commands,
    }
}

fn push_segment(segment: &mut String, commands: &mut Vec<ShellCommand>) {
    let trimmed = segment.trim();
    if !trimmed.is_empty()
        && let Some(name) = trimmed.split_whitespace().next()
    {
        commands.push(ShellCommand {
            name: name.to_string(),
            full: trimmed.to_string(),
        });
    }
    segment.clear();
}

// ────────────────────────────────────────────────────────────────
// Scope matching
// ────────────────────────────────────────────────────────────────

/// Check if any of the extracted subcommands match the given scope pattern.
///
/// Matching semantics depend on where the `*` wildcard appears:
/// - `*` alone — matches everything
/// - `ls *` (space before `*`) — matches `ls` and `ls -a` but not `lsof`
/// - `git commit *` — matches `git commit -m "msg"` (word boundary)
/// - `ls*` (no space before `*`) — matches `lsof`, `ls`, `ls -a` (prefix/glob)
/// - `rm` (no wildcard) — matches exactly `rm`
/// - `git * amend` — matches `git commit amend` (middle wildcard matches zero+ words)
///
/// When `prefix_bare` is true, a bare pattern without wildcards (e.g. `rm`)
/// uses word-boundary prefix matching — `rm` matches `rm -rf /`.  When false,
/// bare patterns require an exact match — `rm` only matches `rm`.
///
/// Allow rules should pass `prefix_bare: false` (strict), while deny/ask rules
/// should pass `prefix_bare: true` (broad) so that denying `rm` also blocks
/// `rm -rf /`.
pub(crate) fn any_subcommand_matches(
    subcommands: &[ShellCommand],
    prefix_bare: bool,
    scope: &str,
) -> bool {
    let scope = scope.trim();

    if scope.is_empty() || scope == "*" {
        return true;
    }

    if let Some(prefix) = scope.strip_suffix(" *") {
        // Word-boundary matching: `ls *` matches `ls` and `ls -a` but not `lsof`
        return subcommands.iter().any(|cmd| {
            if prefix.is_empty() {
                return true;
            }
            let cmd_words: Vec<&str> = cmd.full.split_whitespace().collect();
            let prefix_words: Vec<&str> = prefix.split_whitespace().collect();
            cmd_words.len() >= prefix_words.len()
                && cmd_words[..prefix_words.len()] == prefix_words[..]
        });
    }

    if let Some(prefix) = scope.strip_suffix('*') {
        // Prefix/glob matching: `ls*` matches `lsof`, `ls`, etc.
        return subcommands.iter().any(|cmd| cmd.full.starts_with(prefix));
    }

    if scope.contains('*') {
        // Middle wildcard: `git * amend` — each `*` matches zero or more words
        return subcommands
            .iter()
            .any(|cmd| scope_matches_words(scope, cmd.full.split_whitespace().collect()));
    }

    // No wildcard: exact or prefix depending on context
    let scope_words: Vec<&str> = scope.split_whitespace().collect();
    subcommands.iter().any(|cmd| {
        let cmd_words: Vec<&str> = cmd.full.split_whitespace().collect();
        if prefix_bare {
            cmd_words.len() >= scope_words.len()
                && cmd_words[..scope_words.len()] == scope_words[..]
        } else {
            cmd_words == scope_words
        }
    })
}

/// Match a scope pattern containing `*` wildcards against a sequence of words.
/// Each `*` matches zero or more words. Consecutive `*` collapse into one.
fn scope_matches_words(scope: &str, words: Vec<&str>) -> bool {
    let parts: Vec<&str> = scope.split('*').collect();
    if parts.len() == 1 {
        // No wildcard (shouldn't reach here, but handle it)
        let scope_words: Vec<&str> = scope.split_whitespace().collect();
        return words.len() >= scope_words.len() && words[..scope_words.len()] == scope_words[..];
    }

    // Each segment between * is a sequence of literal words that must appear in order.
    // Walk through `words` consuming segments left to right.
    let mut word_idx = 0;

    for (i, part) in parts.iter().enumerate() {
        let segment_words: Vec<&str> = part.split_whitespace().collect();
        if segment_words.is_empty() {
            continue;
        }

        // Find the segment words starting from word_idx
        if i == 0 {
            // First segment must match at the start
            if words.len() < segment_words.len()
                || words[..segment_words.len()] != segment_words[..]
            {
                return false;
            }
            word_idx = segment_words.len();
        } else if i == parts.len() - 1 {
            // Last segment must match at the end
            if words.len() - word_idx < segment_words.len() {
                return false;
            }
            let start = words.len() - segment_words.len();
            return words[start..] == segment_words[..];
        } else {
            // Middle segment: find it anywhere after word_idx
            let found = find_subslice(&words[word_idx..], &segment_words);
            match found {
                Some(idx) => word_idx += idx + segment_words.len(),
                None => return false,
            }
        }
    }

    true
}

/// Find the first occurrence of `needle` as a contiguous subsequence in `haystack`.
fn find_subslice(haystack: &[&str], needle: &[&str]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    if haystack.len() < needle.len() {
        return None;
    }
    (0..=haystack.len() - needle.len()).find(|&i| haystack[i..i + needle.len()] == needle[..])
}

// ────────────────────────────────────────────────────────────────
// Tests
// ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn names(cmds: &[ShellCommand]) -> Vec<&str> {
        cmds.iter().map(|c| c.name.as_str()).collect()
    }

    fn fulls(cmds: &[ShellCommand]) -> Vec<&str> {
        cmds.iter().map(|c| c.full.as_str()).collect()
    }

    #[test]
    fn simple_command() {
        let result = parse_shell_command("ls -la /tmp", ShellKind::Posix);
        assert_eq!(names(&result.subcommands), vec!["ls"]);
        assert_eq!(fulls(&result.subcommands), vec!["ls -la /tmp"]);
    }

    #[test]
    fn pipeline() {
        let result = parse_shell_command("cat file.txt | grep foo | wc -l", ShellKind::Posix);
        assert_eq!(names(&result.subcommands), vec!["cat", "grep", "wc"]);
    }

    #[test]
    fn command_chaining() {
        let result = parse_shell_command("git add . && git commit -m 'hi'", ShellKind::Posix);
        assert_eq!(names(&result.subcommands), vec!["git", "git"]);
        assert_eq!(
            fulls(&result.subcommands),
            vec!["git add .", "git commit -m 'hi'"]
        );
    }

    #[cfg(feature = "tree-sitter")]
    #[test]
    fn command_substitution() {
        let result = parse_shell_command("echo $(git rev-parse HEAD)", ShellKind::Posix);
        let n = names(&result.subcommands);
        assert!(n.contains(&"echo"), "should contain echo: {n:?}");
        assert!(n.contains(&"git"), "should contain git: {n:?}");
    }

    #[cfg(feature = "tree-sitter")]
    #[test]
    fn backtick_substitution() {
        let result = parse_shell_command("echo `date`", ShellKind::Posix);
        let n = names(&result.subcommands);
        assert!(n.contains(&"echo"), "should contain echo: {n:?}");
        assert!(n.contains(&"date"), "should contain date: {n:?}");
    }

    #[cfg(feature = "tree-sitter")]
    #[test]
    fn subshell() {
        let result = parse_shell_command("(cd /tmp && ls)", ShellKind::Posix);
        assert_eq!(names(&result.subcommands), vec!["cd", "ls"]);
    }

    #[test]
    fn semicolon_separated() {
        let result = parse_shell_command("echo hello; echo world", ShellKind::Posix);
        assert_eq!(names(&result.subcommands), vec!["echo", "echo"]);
    }

    #[cfg(feature = "tree-sitter")]
    #[test]
    fn for_loop() {
        let result = parse_shell_command("for f in *.txt; do cat $f; done", ShellKind::Posix);
        assert!(names(&result.subcommands).contains(&"cat"));
    }

    #[cfg(feature = "tree-sitter")]
    #[test]
    fn if_statement() {
        let result = parse_shell_command(
            "if [ -f foo ]; then cat foo; else echo nope; fi",
            ShellKind::Posix,
        );
        let n = names(&result.subcommands);
        assert!(n.contains(&"cat"), "should contain cat: {n:?}");
        assert!(n.contains(&"echo"), "should contain echo: {n:?}");
    }

    #[test]
    fn scope_matching_wildcard() {
        let commands = vec![
            ShellCommand {
                name: "git".into(),
                full: "git commit -m msg".into(),
            },
            ShellCommand {
                name: "npm".into(),
                full: "npm test".into(),
            },
        ];
        assert!(any_subcommand_matches(&commands, true, "*"));
    }

    #[test]
    fn scope_matching_prefix() {
        let commands = vec![
            ShellCommand {
                name: "git".into(),
                full: "git commit -m msg".into(),
            },
            ShellCommand {
                name: "npm".into(),
                full: "npm test".into(),
            },
        ];
        assert!(any_subcommand_matches(&commands, true, "git commit *"));
        assert!(!any_subcommand_matches(&commands, true, "git push *"));
        assert!(!any_subcommand_matches(&commands, true, "git push"));
        assert!(any_subcommand_matches(&commands, true, "npm *"));
        assert!(any_subcommand_matches(&commands, true, "npm test"));

        // prefix_bare=true: bare "git commit" prefix-matches "git commit -m msg" (deny/ask)
        assert!(any_subcommand_matches(&commands, true, "git commit"));
        // prefix_bare=false: bare "git commit" does NOT match "git commit -m msg" (allow)
        assert!(!any_subcommand_matches(&commands, false, "git commit"));
        // Exact match works in both modes when command has no extra args
        assert!(any_subcommand_matches(&commands, false, "npm test"));
    }

    #[test]
    fn scope_word_boundary_vs_glob() {
        let commands = vec![
            ShellCommand {
                name: "ls".into(),
                full: "ls -a".into(),
            },
            ShellCommand {
                name: "lsof".into(),
                full: "lsof -i :3000".into(),
            },
        ];
        // `ls *` — word boundary: matches `ls -a` but not `lsof`
        assert!(any_subcommand_matches(&commands, true, "ls *"));
        assert!(!any_subcommand_matches(&commands, true, "cat *"));
        assert!(any_subcommand_matches(&commands, true, "lsof *"));

        // `ls*` — glob/prefix: matches both `ls -a` and `lsof`
        assert!(any_subcommand_matches(&commands, true, "ls*"));
    }

    #[test]
    fn scope_exact_match() {
        let commands = vec![ShellCommand {
            name: "ls".into(),
            full: "ls".into(),
        }];
        assert!(any_subcommand_matches(&commands, true, "ls"));
        assert!(!any_subcommand_matches(&commands, true, "cat"));
    }

    #[cfg(feature = "tree-sitter")]
    #[test]
    fn nested_substitution() {
        let result = parse_shell_command(
            "echo \"Result: $(git log --oneline | head -1)\"",
            ShellKind::Posix,
        );
        let n = names(&result.subcommands);
        assert!(n.contains(&"echo"), "should contain echo: {n:?}");
        assert!(n.contains(&"git"), "should contain git: {n:?}");
        assert!(n.contains(&"head"), "should contain head: {n:?}");
    }

    #[test]
    fn fallback_splits_correctly() {
        let result = parse_shell_command("ls && cat foo || echo fail", ShellKind::Other);
        let n = names(&result.subcommands);
        assert!(n.contains(&"ls"), "should contain ls: {n:?}");
        assert!(n.contains(&"cat"), "should contain cat: {n:?}");
        assert!(n.contains(&"echo"), "should contain echo: {n:?}");
    }

    #[test]
    fn fish_simple_command() {
        let result = parse_shell_command("ls -la /tmp", ShellKind::Fish);
        assert_eq!(names(&result.subcommands), vec!["ls"]);
    }

    #[test]
    fn fish_conditional() {
        let result = parse_shell_command("git add .; and git commit -m hi", ShellKind::Fish);
        let n = names(&result.subcommands);
        assert!(n.contains(&"git"), "should contain git: {n:?}");
    }

    #[cfg(feature = "tree-sitter")]
    #[test]
    fn fish_command_substitution() {
        let result = parse_shell_command("echo (date)", ShellKind::Fish);
        let n = names(&result.subcommands);
        assert!(n.contains(&"echo"), "should contain echo: {n:?}");
        assert!(n.contains(&"date"), "should contain date: {n:?}");
    }

    #[cfg(feature = "tree-sitter")]
    #[test]
    fn variable_assignment_excluded() {
        let result = parse_shell_command("FOO=bar ls -la /tmp", ShellKind::Posix);
        assert_eq!(names(&result.subcommands), vec!["ls"]);
        assert_eq!(fulls(&result.subcommands), vec!["ls -la /tmp"]);
    }

    #[cfg(feature = "tree-sitter")]
    #[test]
    fn variable_assignment_multiple() {
        let result = parse_shell_command("A=1 B=2 git status", ShellKind::Posix);
        assert_eq!(names(&result.subcommands), vec!["git"]);
        assert_eq!(fulls(&result.subcommands), vec!["git status"]);
    }

    #[test]
    fn fallback_double_ampersand_and_pipe_pipe() {
        let result = parse_shell_command("ls && cat foo || echo fail", ShellKind::Other);
        assert_eq!(names(&result.subcommands), vec!["ls", "cat", "echo"]);
        assert_eq!(
            fulls(&result.subcommands),
            vec!["ls", "cat foo", "echo fail"]
        );
    }

    #[test]
    fn fallback_pipe_without_double() {
        let result = parse_shell_command("ls | grep foo", ShellKind::Other);
        assert_eq!(names(&result.subcommands), vec!["ls", "grep"]);
        assert_eq!(fulls(&result.subcommands), vec!["ls", "grep foo"]);
    }

    #[test]
    fn scope_middle_wildcard() {
        let commands = vec![ShellCommand {
            name: "git".into(),
            full: "git commit -m amend".into(),
        }];
        assert!(any_subcommand_matches(&commands, true, "git * amend"));
        assert!(any_subcommand_matches(
            &commands,
            true,
            "git commit * amend"
        ));
        assert!(!any_subcommand_matches(&commands, true, "git push * amend"));
    }

    #[test]
    fn scope_middle_wildcard_zero_words() {
        let commands = vec![ShellCommand {
            name: "git".into(),
            full: "git commit".into(),
        }];
        // `*` matches zero words, so `git * commit` should match `git commit`
        assert!(any_subcommand_matches(&commands, true, "git * commit"));
    }

    #[test]
    fn scope_leading_wildcard() {
        let commands = vec![ShellCommand {
            name: "docker".into(),
            full: "docker run --rm alpine".into(),
        }];
        assert!(any_subcommand_matches(&commands, true, "* alpine"));
        assert!(!any_subcommand_matches(&commands, true, "* ubuntu"));
    }

    #[test]
    fn scope_multiple_wildcards() {
        let commands = vec![ShellCommand {
            name: "git".into(),
            full: "git rebase -i HEAD~5".into(),
        }];
        assert!(any_subcommand_matches(&commands, true, "git * -i * HEAD~5"));
        assert!(!any_subcommand_matches(
            &commands,
            true,
            "git * -i * HEAD~10"
        ));
    }
}

#[cfg(all(test, feature = "tree-sitter"))]
mod adversarial {
    use super::*;

    fn cmd_names(cmds: &[ShellCommand]) -> Vec<&str> {
        cmds.iter().map(|c| c.name.as_str()).collect()
    }

    /// Helper: assert that parsing POSIX extracts all expected command names
    fn assert_posix(code: &str, expected: &[&str]) {
        let result = parse_shell_command(code, ShellKind::Posix);
        let mut got: Vec<&str> = result.subcommands.iter().map(|c| c.name.as_str()).collect();
        got.sort();
        let mut want: Vec<&str> = expected.to_vec();
        want.sort();
        assert_eq!(
            got, want,
            "POSIX parse of {:?}:\n  got:  {:?}\n  want: {:?}",
            code, got, want
        );
    }

    fn assert_fish(code: &str, expected: &[&str]) {
        let result = parse_shell_command(code, ShellKind::Fish);
        let mut got: Vec<&str> = result.subcommands.iter().map(|c| c.name.as_str()).collect();
        got.sort();
        let mut want: Vec<&str> = expected.to_vec();
        want.sort();
        assert_eq!(
            got, want,
            "Fish parse of {:?}:\n  got:  {:?}\n  want: {:?}",
            code, got, want
        );
    }

    // ────────────────────────────────────────────────────────────
    // Level 1: Basic compounds
    // ────────────────────────────────────────────────────────────

    #[test]
    fn a01_triple_chain() {
        assert_posix("a && b && c", &["a", "b", "c"]);
    }

    #[test]
    fn a02_or_chain() {
        assert_posix("a || b || c", &["a", "b", "c"]);
    }

    #[test]
    fn a03_mixed_chain() {
        assert_posix("a && b || c && d", &["a", "b", "c", "d"]);
    }

    #[test]
    fn a04_long_pipeline() {
        assert_posix(
            "cat foo | grep bar | awk '{print $1}' | sort | uniq -c",
            &["cat", "grep", "awk", "sort", "uniq"],
        );
    }

    #[test]
    fn a05_semicolons() {
        assert_posix("a; b; c; d", &["a", "b", "c", "d"]);
    }

    // ────────────────────────────────────────────────────────────
    // Level 2: Nested substitution
    // ────────────────────────────────────────────────────────────

    #[test]
    fn a06_nested_dollar() {
        assert_posix(
            "echo $(basename $(dirname /foo/bar))",
            &["echo", "basename", "dirname"],
        );
    }

    #[test]
    fn a07_deeply_nested() {
        // 4 nested echos, all should be extracted
        assert_posix(
            "echo $(echo $(echo $(echo deep)))",
            &["echo", "echo", "echo", "echo"],
        );
    }

    #[test]
    fn a08_backtick_in_echo() {
        assert_posix("echo `hostname`", &["echo", "hostname"]);
    }

    #[test]
    fn a09_mixed_substitutions() {
        assert_posix("echo $(date) `uname`", &["echo", "date", "uname"]);
    }

    // ────────────────────────────────────────────────────────────
    // Level 3: Subshells and grouping
    // ────────────────────────────────────────────────────────────

    #[test]
    fn a10_subshell_chain() {
        assert_posix("(cd /tmp && ls -la)", &["cd", "ls"]);
    }

    #[test]
    fn a11_nested_subshells() {
        assert_posix("( (inner_cmd) )", &["inner_cmd"]);
    }

    #[test]
    fn a12_brace_group() {
        assert_posix("{ cd /tmp; ls; }", &["cd", "ls"]);
    }

    // ────────────────────────────────────────────────────────────
    // Level 4: Variable assignments
    // ────────────────────────────────────────────────────────────

    #[test]
    fn a13_single_var_assignment() {
        let result = parse_shell_command("FOO=bar ls", ShellKind::Posix);
        assert_eq!(cmd_names(&result.subcommands), &["ls"]);
        assert_eq!(result.subcommands[0].full, "ls");
    }

    #[test]
    fn a14_multiple_var_assignments() {
        let result = parse_shell_command("A=1 B=2 C=3 git status", ShellKind::Posix);
        assert_eq!(cmd_names(&result.subcommands), &["git"]);
        assert_eq!(result.subcommands[0].full, "git status");
    }

    #[test]
    fn a15_var_assignment_no_command() {
        // Variable assignment only — no command to extract
        assert_posix("FOO=bar", &[]);
    }

    #[test]
    fn a16_var_assignment_in_pipeline() {
        assert_posix("FOO=bar ls | BAZ=qux grep foo", &["ls", "grep"]);
    }

    // ────────────────────────────────────────────────────────────
    // Level 5: Control flow
    // ────────────────────────────────────────────────────────────

    #[test]
    fn a17_if_then_else() {
        assert_posix(
            "if [ -f foo ]; then cat foo; else echo missing; fi",
            &["cat", "echo"],
        );
    }

    #[test]
    fn a18_elif_chain() {
        // Two cat commands (then + elif branch), one echo (else branch).
        // [ is part of the test_condition, not extracted as a command.
        assert_posix(
            "if [ -f a ]; then cat a; elif [ -f b ]; then cat b; else echo none; fi",
            &["cat", "cat", "echo"],
        );
    }

    #[test]
    fn a19_for_loop() {
        assert_posix("for f in *.txt; do cat \"$f\"; done", &["cat"]);
    }

    #[test]
    fn a20_while_loop() {
        // read in the condition is a real command
        assert_posix(
            "while read line; do echo \"$line\"; done < input.txt",
            &["echo", "read"],
        );
    }

    #[test]
    fn f07_if_statement() {
        // test in if-condition is a real command
        assert_fish(
            "if test -f foo; cat foo; else; echo missing; end",
            &["cat", "echo", "test"],
        );
    }

    #[test]
    fn f09_while_loop() {
        // `true` in the condition is a real command
        assert_fish(
            "while true; echo tick; sleep 1; end",
            &["echo", "sleep", "true"],
        );
    }

    // ────────────────────────────────────────────────────────────
    // Level 6: Redirections
    // ────────────────────────────────────────────────────────────

    #[test]
    fn a23_redirect_out() {
        assert_posix("ls > output.txt", &["ls"]);
    }

    #[test]
    fn a24_redirect_append() {
        assert_posix("ls >> output.txt 2>&1", &["ls"]);
    }

    #[test]
    fn a25_here_string() {
        assert_posix("grep foo <<< \"hello world\"", &["grep"]);
    }

    #[test]
    fn a26_redirect_in_pipeline() {
        assert_posix("cat < input.txt | sort | uniq", &["cat", "sort", "uniq"]);
    }

    #[test]
    fn a27_process_substitution() {
        assert_posix(
            "diff <(sort a.txt) <(sort b.txt)",
            &["diff", "sort", "sort"],
        );
    }

    // ────────────────────────────────────────────────────────────
    // Level 7: Function definitions
    // ────────────────────────────────────────────────────────────

    #[test]
    fn a28_function_def() {
        assert_posix("foo() { echo hello; }", &["echo"]);
    }

    #[test]
    fn a29_function_with_subshell() {
        assert_posix(
            "build() { cargo build && cargo test; }",
            &["cargo", "cargo"],
        );
    }

    // ────────────────────────────────────────────────────────────
    // Level 8: Edge cases — empties, weird quoting
    // ────────────────────────────────────────────────────────────

    #[test]
    fn a30_empty_string() {
        let result = parse_shell_command("", ShellKind::Posix);
        assert!(result.subcommands.is_empty());
    }

    #[test]
    fn a31_whitespace_only() {
        let result = parse_shell_command("   \t  \n  ", ShellKind::Posix);
        assert!(result.subcommands.is_empty());
    }

    #[test]
    fn a32_single_command_no_args() {
        assert_posix("ls", &["ls"]);
    }

    #[test]
    fn a33_command_with_single_quotes() {
        assert_posix("echo 'hello world'", &["echo"]);
    }

    #[test]
    fn a34_command_with_double_quotes() {
        assert_posix("echo \"hello world\"", &["echo"]);
    }

    #[test]
    fn a35_escaped_spaces() {
        // ls\ -la is a single word in bash, not "ls" with flag "-la"
        assert_posix("ls\\ -la", &["ls\\ -la"]);
    }

    #[test]
    fn a36_command_with_dollar_var() {
        assert_posix("echo $HOME/.bashrc", &["echo"]);
    }

    // ────────────────────────────────────────────────────────────
    // Level 9: Background jobs and coproc
    // ────────────────────────────────────────────────────────────

    #[test]
    fn a37_background_job() {
        assert_posix("sleep 10 &", &["sleep"]);
    }

    #[test]
    fn a38_background_chain() {
        assert_posix("sleep 10 && echo done &", &["sleep", "echo"]);
    }

    // ────────────────────────────────────────────────────────────
    // Level 10: Real-world complex commands
    // ────────────────────────────────────────────────────────────

    #[test]
    fn a39_docker_build_and_run() {
        assert_posix(
            "docker build -t app . && docker run --rm app npm test",
            &["docker", "docker"],
        );
    }

    #[test]
    fn a40_git_rebase_interactive() {
        assert_posix(
            "GIT_SEQUENCE_EDITOR=\"sed -i 's/pick/reword/'\" git rebase -i HEAD~5",
            &["git"],
        );
    }

    #[test]
    fn a41_find_with_exec() {
        // tree-sitter-bash does not parse -exec body as commands — only `find` is extracted.
        // This is a known limitation: args to -exec/-execdir are opaque to the parser.
        assert_posix("find . -name '*.rs' -exec grep -l 'unsafe' {} +", &["find"]);
    }

    #[test]
    fn a42_curl_pipe_sh() {
        assert_posix(
            "curl -sSL https://example.com/install.sh | bash",
            &["curl", "bash"],
        );
    }

    #[test]
    fn a43_xargs() {
        assert_posix("find . -name '*.tmp' | xargs rm -f", &["find", "xargs"]);
    }

    #[test]
    fn a44_npm_script_chain() {
        assert_posix(
            "npm run build && npm run test && npm run lint",
            &["npm", "npm", "npm"],
        );
    }

    #[test]
    fn a45_make_with_redirect() {
        assert_posix(
            "make -j$(nproc) 2>&1 | tee build.log",
            &["make", "nproc", "tee"],
        );
    }

    #[test]
    fn a46_sudo_chain() {
        assert_posix("sudo apt update && sudo apt upgrade -y", &["sudo", "sudo"]);
    }

    #[test]
    fn a47_here_doc_with_subcommand() {
        assert_posix("cat <<EOF\nhello $(whoami)\nEOF", &["cat", "whoami"]);
    }

    #[test]
    fn a48_eval_with_command() {
        assert_posix("eval \"echo hello\"", &["eval"]);
    }

    #[test]
    fn a49_exec_replace() {
        assert_posix("exec ls", &["exec"]);
    }

    #[test]
    fn a50_source_script() {
        assert_posix("source ~/.bashrc", &["source"]);
    }

    // ────────────────────────────────────────────────────────────
    // Level 11: Fish-specific tests
    // ────────────────────────────────────────────────────────────

    #[test]
    fn f01_simple() {
        assert_fish("ls -la /tmp", &["ls"]);
    }

    #[test]
    fn f02_pipe() {
        assert_fish("cat foo | grep bar | sort", &["cat", "grep", "sort"]);
    }

    #[test]
    fn f03_and() {
        assert_fish("git add .; and git commit -m hi", &["git", "git"]);
    }

    #[test]
    fn f04_or() {
        assert_fish("test -f foo; or echo missing", &["test", "echo"]);
    }

    #[test]
    fn f04_not() {
        // fish parses `not test -f foo` — `not` is a modifier, `test` is the command
        assert_fish("not test -f foo", &["test"]);
    }

    #[test]
    fn f05_command_substitution() {
        assert_fish("echo (date)", &["echo", "date"]);
    }

    #[test]
    fn f06_nested_substitution() {
        assert_fish(
            "echo (basename (dirname /foo/bar))",
            &["echo", "basename", "dirname"],
        );
    }

    #[test]
    fn f06_begin_end() {
        assert_fish("begin; ls; echo done; end", &["ls", "echo"]);
    }

    #[test]
    fn f10_switch() {
        // Two echo commands, one per case branch
        assert_fish(
            "switch $x; case foo; echo foo; case bar; echo bar; end",
            &["echo", "echo"],
        );
    }

    #[test]
    fn f08_for_loop() {
        assert_fish("for f in *.txt; cat $f; end", &["cat"]);
    }

    #[test]
    fn a21_case_statement() {
        // Two echo branches
        assert_posix(
            "case $x in foo) echo foo;; bar) echo bar;; esac",
            &["echo", "echo"],
        );
    }

    #[test]
    fn f11_function_def() {
        assert_fish("function greet; echo hello $argv; end", &["echo"]);
    }

    #[test]
    fn f12_redirect() {
        assert_fish("ls > output.txt", &["ls"]);
    }

    #[test]
    fn f13_redirect_append() {
        assert_fish("ls >> output.txt", &["ls"]);
    }

    #[test]
    fn f14_here_string() {
        assert_fish("grep foo <<< \"hello\"", &["grep"]);
    }

    #[test]
    fn f15_curl_pipe() {
        assert_fish(
            "curl -sSL https://example.com/install.sh | bash",
            &["curl", "bash"],
        );
    }

    #[test]
    fn f16_double_ampersand() {
        assert_fish("git add . && git commit -m hi", &["git", "git"]);
    }

    #[test]
    fn f17_double_pipe() {
        assert_fish("test -f foo || echo missing", &["test", "echo"]);
    }

    #[test]
    fn f18_empty() {
        let result = parse_shell_command("", ShellKind::Fish);
        assert!(result.subcommands.is_empty());
    }

    #[test]
    fn f19_whitespace() {
        let result = parse_shell_command("   ", ShellKind::Fish);
        assert!(result.subcommands.is_empty());
    }

    // ────────────────────────────────────────────────────────────
    // Level 12: Scope matching adversarial
    // ────────────────────────────────────────────────────────────

    #[test]
    fn s01_empty_scope() {
        let commands = vec![ShellCommand {
            name: "ls".into(),
            full: "ls".into(),
        }];
        // Empty scope matches everything (nothing to constrain)
        assert!(any_subcommand_matches(&commands, true, ""));
    }

    #[test]
    fn s03_only_wildcard_space_star() {
        let commands = vec![ShellCommand {
            name: "ls".into(),
            full: "ls".into(),
        }];
        // " *" with empty prefix = match anything
        assert!(any_subcommand_matches(&commands, true, " *"));
    }

    #[test]
    fn s04_glob_matches_empty() {
        let commands = vec![ShellCommand {
            name: "ls".into(),
            full: "ls".into(),
        }];
        // `ls*` matches `ls` (prefix match with nothing after)
        assert!(any_subcommand_matches(&commands, true, "ls*"));
    }

    #[test]
    fn s05_middle_wildcard_empty_match() {
        // `git * commit` matches `git commit` (* = zero words)
        let commands = vec![ShellCommand {
            name: "git".into(),
            full: "git commit".into(),
        }];
        assert!(any_subcommand_matches(&commands, true, "git * commit"));
    }

    #[test]
    fn s06_consecutive_wildcards() {
        // `git ** commit` should behave like `git * commit`
        let commands = vec![ShellCommand {
            name: "git".into(),
            full: "git commit".into(),
        }];
        assert!(any_subcommand_matches(&commands, true, "git ** commit"));
    }

    #[test]
    fn s07_case_sensitivity() {
        let commands = vec![ShellCommand {
            name: "LS".into(),
            full: "LS -la".into(),
        }];
        // Wildcard: case matters
        assert!(!any_subcommand_matches(&commands, true, "ls *"));
        assert!(any_subcommand_matches(&commands, true, "LS *"));
        // prefix_bare=true: bare "LS" prefix-matches "LS -la"
        assert!(!any_subcommand_matches(&commands, true, "ls"));
        assert!(any_subcommand_matches(&commands, true, "LS"));
        // prefix_bare=false: bare "LS" does NOT match "LS -la"
        assert!(!any_subcommand_matches(&commands, false, "LS"));
    }

    #[test]
    fn s08_multi_word_exact_no_subcommand() {
        // `git commit` should not match `git commit-amend`
        let commands = vec![ShellCommand {
            name: "git".into(),
            full: "git commit-amend".into(),
        }];
        assert!(!any_subcommand_matches(&commands, true, "git commit"));
    }
}
