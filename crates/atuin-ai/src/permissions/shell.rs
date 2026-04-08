use tree_sitter::{Parser, Tree};

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
    match shell {
        ShellKind::Posix => parse_posix(code),
        ShellKind::Fish => parse_fish(code),
        ShellKind::Other => parse_fallback(code),
    }
}

// ────────────────────────────────────────────────────────────────
// POSIX (bash/zsh/sh) parser
// ────────────────────────────────────────────────────────────────

fn bash_parser() -> Parser {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_bash::LANGUAGE.into())
        .expect("failed to set bash language");
    parser
}

fn parse_posix(code: &str) -> ParsedShellCommand {
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
    "concatenation",
];

fn walk_bash_tree(tree: &Tree, source: &[u8], commands: &mut Vec<ShellCommand>) {
    walk_bash_node(tree.root_node(), source, commands);
}

fn walk_bash_node(node: tree_sitter::Node, source: &[u8], commands: &mut Vec<ShellCommand>) {
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
fn extract_bash_command(node: tree_sitter::Node, source: &[u8]) -> Option<ShellCommand> {
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

fn parse_fish(code: &str) -> ParsedShellCommand {
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

fn walk_fish_node(node: tree_sitter::Node, source: &[u8], commands: &mut Vec<ShellCommand>) {
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

fn extract_fish_command(node: tree_sitter::Node, source: &[u8]) -> Option<ShellCommand> {
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
    if !trimmed.is_empty() {
        if let Some(name) = trimmed.split_whitespace().next() {
            commands.push(ShellCommand {
                name: name.to_string(),
                full: trimmed.to_string(),
            });
        }
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
pub(crate) fn any_subcommand_matches(subcommands: &[ShellCommand], scope: &str) -> bool {
    let scope = scope.trim();

    if scope == "*" {
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

    if scope.ends_with('*') {
        // Prefix/glob matching: `ls*` matches `lsof`, `ls`, etc.
        let prefix = &scope[..scope.len() - 1];
        return subcommands.iter().any(|cmd| cmd.full.starts_with(prefix));
    }

    if scope.contains('*') {
        // Middle wildcard: `git * amend` — each `*` matches zero or more words
        return subcommands
            .iter()
            .any(|cmd| scope_matches_words(scope, cmd.full.split_whitespace().collect()));
    }

    // No wildcard: word-boundary prefix match
    let scope_words: Vec<&str> = scope.split_whitespace().collect();
    subcommands.iter().any(|cmd| {
        let cmd_words: Vec<&str> = cmd.full.split_whitespace().collect();
        cmd_words.len() >= scope_words.len() && cmd_words[..scope_words.len()] == scope_words[..]
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

    #[test]
    fn command_substitution() {
        let result = parse_shell_command("echo $(git rev-parse HEAD)", ShellKind::Posix);
        let n = names(&result.subcommands);
        assert!(n.contains(&"echo"), "should contain echo: {n:?}");
        assert!(n.contains(&"git"), "should contain git: {n:?}");
    }

    #[test]
    fn backtick_substitution() {
        let result = parse_shell_command("echo `date`", ShellKind::Posix);
        let n = names(&result.subcommands);
        assert!(n.contains(&"echo"), "should contain echo: {n:?}");
        assert!(n.contains(&"date"), "should contain date: {n:?}");
    }

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

    #[test]
    fn for_loop() {
        let result = parse_shell_command("for f in *.txt; do cat $f; done", ShellKind::Posix);
        assert!(names(&result.subcommands).contains(&"cat"));
    }

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
        assert!(any_subcommand_matches(&commands, "*"));
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
        assert!(any_subcommand_matches(&commands, "git commit *"));
        assert!(any_subcommand_matches(&commands, "git commit"));
        assert!(!any_subcommand_matches(&commands, "git push *"));
        assert!(!any_subcommand_matches(&commands, "git push"));
        assert!(any_subcommand_matches(&commands, "npm *"));
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
        assert!(any_subcommand_matches(&commands, "ls *"));
        assert!(!any_subcommand_matches(&commands, "cat *"));
        assert!(any_subcommand_matches(&commands, "lsof *"));

        // `ls*` — glob/prefix: matches both `ls -a` and `lsof`
        assert!(any_subcommand_matches(&commands, "ls*"));
    }

    #[test]
    fn scope_exact_match() {
        let commands = vec![ShellCommand {
            name: "ls".into(),
            full: "ls".into(),
        }];
        assert!(any_subcommand_matches(&commands, "ls"));
        assert!(!any_subcommand_matches(&commands, "cat"));
    }

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

    #[test]
    fn fish_command_substitution() {
        let result = parse_shell_command("echo (date)", ShellKind::Fish);
        let n = names(&result.subcommands);
        assert!(n.contains(&"echo"), "should contain echo: {n:?}");
        assert!(n.contains(&"date"), "should contain date: {n:?}");
    }

    #[test]
    fn variable_assignment_excluded() {
        let result = parse_shell_command("FOO=bar ls -la /tmp", ShellKind::Posix);
        assert_eq!(names(&result.subcommands), vec!["ls"]);
        assert_eq!(fulls(&result.subcommands), vec!["ls -la /tmp"]);
    }

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
        assert!(any_subcommand_matches(&commands, "git * amend"));
        assert!(any_subcommand_matches(&commands, "git commit * amend"));
        assert!(!any_subcommand_matches(&commands, "git push * amend"));
    }

    #[test]
    fn scope_middle_wildcard_zero_words() {
        let commands = vec![ShellCommand {
            name: "git".into(),
            full: "git commit".into(),
        }];
        // `*` matches zero words, so `git * commit` should match `git commit`
        assert!(any_subcommand_matches(&commands, "git * commit"));
    }

    #[test]
    fn scope_leading_wildcard() {
        let commands = vec![ShellCommand {
            name: "docker".into(),
            full: "docker run --rm alpine".into(),
        }];
        assert!(any_subcommand_matches(&commands, "* alpine"));
        assert!(!any_subcommand_matches(&commands, "* ubuntu"));
    }

    #[test]
    fn scope_multiple_wildcards() {
        let commands = vec![ShellCommand {
            name: "git".into(),
            full: "git rebase -i HEAD~5".into(),
        }];
        assert!(any_subcommand_matches(&commands, "git * -i * HEAD~5"));
        assert!(!any_subcommand_matches(&commands, "git * -i * HEAD~10"));
    }
}
