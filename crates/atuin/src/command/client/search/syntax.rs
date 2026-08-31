//! Shell syntax highlighting for the interactive history list, via
//! tree-sitter. On platforms where tree-sitter's bundled C doesn't
//! build (see the note in Cargo.toml), commands are left unhighlighted.

use atuin_client::theme::Meaning;

/// Style every byte of `cmd` with a `Syntax*` meaning, parsing with the
/// grammar for the entry's shell. Anything unrecognized (plain arguments,
/// parse errors, shells without a grammar) stays `Base`.
///
/// Rows are re-classified on every redraw while typing or scrolling, so
/// results are memoized; repeat frames cost a hash lookup, not a parse.
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows", target_os = "illumos"))]
#[must_use]
pub fn classify(cmd: &str, shell: Option<&str>) -> Vec<Meaning> {
    use std::cell::RefCell;
    use std::collections::HashMap;

    thread_local! {
        static CACHE: RefCell<HashMap<String, Vec<Meaning>>> = RefCell::new(HashMap::new());
    }

    let key = format!("{}\x1f{}", shell.unwrap_or(""), cmd);
    CACHE.with_borrow_mut(|cache| {
        if cache.len() > 4096 {
            cache.clear();
        }
        cache.entry(key).or_insert_with(|| parse(cmd, shell)).clone()
    })
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows", target_os = "illumos"))]
fn parse(cmd: &str, shell: Option<&str>) -> Vec<Meaning> {
    let mut meanings = vec![Meaning::Base; cmd.len()];

    let language: tree_sitter::Language = match shell {
        Some("fish") => tree_sitter_fish::language(),
        Some("powershell" | "pwsh") => tree_sitter_powershell::LANGUAGE.into(),
        // POSIX-ish shells; entries from before the shell was recorded
        // get bash as the best guess
        None | Some("bash" | "zsh" | "sh") => tree_sitter_bash::LANGUAGE.into(),
        // nu, xonsh, ...: no grammar available
        Some(_) => return meanings,
    };

    let mut parser = tree_sitter::Parser::new();
    if parser.set_language(&language).is_ok()
        && let Some(tree) = parser.parse(cmd, None)
    {
        if language.name() == Some("powershell") {
            highlight_powershell(tree.root_node(), cmd.as_bytes(), &mut meanings);
        } else {
            walk(tree.root_node(), cmd.as_bytes(), &mut meanings);
        }
    }
    meanings
}

#[cfg(not(any(
    target_os = "linux",
    target_os = "macos",
    target_os = "windows",
    target_os = "illumos"
)))]
pub fn classify(cmd: &str, _shell: Option<&str>) -> Vec<Meaning> {
    vec![Meaning::Base; cmd.len()]
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows", target_os = "illumos"))]
fn walk(node: tree_sitter::Node, src: &[u8], meanings: &mut [Meaning]) {
    let meaning = match node.kind() {
        "comment" => Some(Meaning::SyntaxComment),
        "command_name" => Some(Meaning::SyntaxCommand),
        "string"
        | "raw_string"
        | "ansi_c_string"
        | "heredoc_body"
        | "single_quote_string"
        | "double_quote_string" => Some(Meaning::SyntaxString),
        "simple_expansion" | "expansion" | "variable_assignment" | "variable_expansion" => {
            Some(Meaning::SyntaxVariable)
        }
        "word" if src.get(node.start_byte()) == Some(&b'-') => Some(Meaning::SyntaxFlag),
        // Anonymous tokens made of operator characters: `|`, `&&`, `;`, `$(`, ...
        k if !node.is_named()
            && !k.is_empty()
            && k.bytes().all(|b| b"|&;<>(){}$`".contains(&b)) =>
        {
            Some(Meaning::SyntaxOperator)
        }
        _ => None,
    };
    if let Some(meaning) = meaning
        && let Some(range) = meanings.get_mut(node.byte_range())
    {
        range.fill(meaning);
    }

    // Fish has no command_name node kind; the command's `name` field points at
    // a plain word (in bash it points at the command_name, filled above too).
    if node.kind() == "command"
        && let Some(name) = node.child_by_field_name("name")
        && let Some(range) = meanings.get_mut(name.byte_range())
    {
        range.fill(Meaning::SyntaxCommand);
    }

    // An expansion is uniformly a variable; don't let its `$`/`${`/`}` child
    // tokens overwrite it as operators.
    if matches!(node.kind(), "simple_expansion" | "expansion" | "variable_expansion") {
        return;
    }

    // Descend so nested nodes refine their parent's color, e.g. `$var` inside
    // a double-quoted string, or the string value in `FOO="bar"`.
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk(child, src, meanings);
    }
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows", target_os = "illumos"))]
fn highlight_powershell(node: tree_sitter::Node, src: &[u8], meanings: &mut [Meaning]) {
    // PowerShell has a different syntax than most shells, so it's handled separately.

    static HIGHLIGHTS_QUERY: std::sync::LazyLock<tree_sitter::Query> =
        std::sync::LazyLock::new(|| {
            let language: tree_sitter::Language = tree_sitter_powershell::LANGUAGE.into();
            tree_sitter::Query::new(&language, include_str!("highlights/powershell.scm"))
                .expect("invalid PowerShell highlights query")
        });

    use tree_sitter::StreamingIterator;

    let mut cursor = tree_sitter::QueryCursor::new();
    let mut captures = cursor.captures(&HIGHLIGHTS_QUERY, node, src);

    while let Some((m, capture_index)) = captures.next() {
        let capture = m.captures[*capture_index];
        let capture_name = HIGHLIGHTS_QUERY.capture_names()[capture.index as usize];

        let meaning = match capture_name {
            "base" => Meaning::Base,
            "command" => Meaning::SyntaxCommand,
            "flag" => Meaning::SyntaxFlag,
            "string" => Meaning::SyntaxString,
            "variable" | "keyword" => Meaning::SyntaxVariable,
            "operator" => Meaning::SyntaxOperator,
            "comment" => Meaning::SyntaxComment,
            _ => continue, // currently ignored: number
        };

        if let Some(range) = meanings.get_mut(capture.node.byte_range()) {
            range.fill(meaning);
        }
    }
}

#[cfg(all(
    test,
    any(target_os = "linux", target_os = "macos", target_os = "windows", target_os = "illumos")
))]
mod tests {
    use rstest::rstest;

    use super::{Meaning, classify};

    /// Render the classification as one char per byte for compact assertions.
    fn render_shell(cmd: &str, shell: Option<&str>) -> String {
        classify(cmd, shell)
            .iter()
            .map(|m| match m {
                Meaning::SyntaxCommand => 'c',
                Meaning::SyntaxFlag => 'f',
                Meaning::SyntaxString => 's',
                Meaning::SyntaxOperator => 'o',
                Meaning::SyntaxVariable => 'v',
                Meaning::SyntaxComment => '#',
                _ => 'a',
            })
            .collect()
    }

    #[rstest]
    #[case::simple_command("git commit -m 'hi'", None, "cccaaaaaaaaffassss")]
    #[case::pipe("cat foo | grep bar", None, "cccaaaaaoaccccaaaa")]
    #[case::and_list("true && false", None, "ccccaooaccccc")]
    #[case::env_assignment("FOO=bar make", None, "vvvvvvvacccc")]
    #[case::variables("echo $HOME ${USER}x", None, "ccccavvvvvavvvvvvva")]
    #[case::variable_in_string(r#"echo "hi $USER""#, None, "ccccassssvvvvvs")]
    #[case::comment("ls # list", None, "cca######")]
    #[case::comment_no_space("echo foo#bar", None, "ccccaaaaaaaa")]
    #[case::fish_set("set -x PATH $PATH", Some("fish"), "cccaffaaaaaavvvvv")]
    #[case::fish_subshell("echo (date) | grep foo", Some("fish"), "ccccaoccccoaoaccccaaaa")]
    #[case::fish_string(r#"echo "hi $name""#, Some("fish"), "ccccassssvvvvvs")]
    #[case::powershell("$v = rg -i 'foo' $f", Some("powershell"), "vvaoaccaffasssssavv")]
    #[case::zsh_uses_bash("ls -la", Some("zsh"), "ccafff")]
    #[case::nu_plain("ls -la", Some("nu"), "aaaaaa")]
    fn classify_renders(#[case] cmd: &str, #[case] shell: Option<&str>, #[case] expected: &str) {
        assert_eq!(render_shell(cmd, shell), expected);
    }

    #[rstest]
    fn odd_inputs_do_not_panic(
        // unterminated string, non-bash syntax, empty, multibyte
        #[values("echo 'oops", "if (= 1 2) { }", "", "echo héllo")] cmd: &str,
        #[values(None, Some("fish"), Some("nu"), Some("powershell"))] shell: Option<&str>,
    ) {
        assert_eq!(classify(cmd, shell).len(), cmd.len());
    }
}
