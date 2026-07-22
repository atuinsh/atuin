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
#[cfg(any(target_os = "linux", target_os = "macos"))]
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
        cache
            .entry(key)
            .or_insert_with(|| parse(cmd, shell))
            .clone()
    })
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn parse(cmd: &str, shell: Option<&str>) -> Vec<Meaning> {
    let mut meanings = vec![Meaning::Base; cmd.len()];

    let language: tree_sitter::Language = match shell {
        Some("fish") => tree_sitter_fish::language(),
        // POSIX-ish shells; entries from before the shell was recorded
        // get bash as the best guess
        None | Some("bash" | "zsh" | "sh") => tree_sitter_bash::LANGUAGE.into(),
        // nu, xonsh, powershell, ...: no grammar available
        Some(_) => return meanings,
    };

    let mut parser = tree_sitter::Parser::new();
    if parser.set_language(&language).is_ok()
        && let Some(tree) = parser.parse(cmd, None)
    {
        walk(tree.root_node(), cmd.as_bytes(), &mut meanings);
    }
    meanings
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub fn classify(cmd: &str, _shell: Option<&str>) -> Vec<Meaning> {
    vec![Meaning::Base; cmd.len()]
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
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
    if matches!(
        node.kind(),
        "simple_expansion" | "expansion" | "variable_expansion"
    ) {
        return;
    }

    // Descend so nested nodes refine their parent's color, e.g. `$var` inside
    // a double-quoted string, or the string value in `FOO="bar"`.
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk(child, src, meanings);
    }
}

#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
mod tests {
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

    fn render(cmd: &str) -> String {
        render_shell(cmd, None)
    }

    fn render_fish(cmd: &str) -> String {
        render_shell(cmd, Some("fish"))
    }

    #[test]
    fn simple_command() {
        assert_eq!(render("git commit -m 'hi'"), "cccaaaaaaaaffassss");
    }

    #[test]
    fn pipes_and_lists_start_new_commands() {
        assert_eq!(render("cat foo | grep bar"), "cccaaaaaoaccccaaaa");
        assert_eq!(render("true && false"), "ccccaooaccccc");
    }

    #[test]
    fn env_assignment_prefix() {
        assert_eq!(render("FOO=bar make"), "vvvvvvvacccc");
    }

    #[test]
    fn variables() {
        assert_eq!(render("echo $HOME ${USER}x"), "ccccavvvvvavvvvvvva");
    }

    #[test]
    fn variable_inside_string_refines_string() {
        assert_eq!(render(r#"echo "hi $USER""#), "ccccassssvvvvvs");
    }

    #[test]
    fn comment() {
        assert_eq!(render("ls # list"), "cca######");
        assert_eq!(render("echo foo#bar"), "ccccaaaaaaaa");
    }

    #[test]
    fn fish_uses_the_fish_grammar() {
        assert_eq!(render_fish("set -x PATH $PATH"), "cccaffaaaaaavvvvv");
        assert_eq!(
            render_fish("echo (date) | grep foo"),
            "ccccaoccccoaoaccccaaaa"
        );
        assert_eq!(render_fish(r#"echo "hi $name""#), "ccccassssvvvvvs");
    }

    #[test]
    fn zsh_uses_the_bash_grammar() {
        assert_eq!(render_shell("ls -la", Some("zsh")), "ccafff");
    }

    #[test]
    fn shells_without_a_grammar_stay_plain() {
        assert_eq!(render_shell("ls -la", Some("nu")), "aaaaaa");
        assert_eq!(render_shell("ls -la", Some("powershell")), "aaaaaa");
    }

    #[test]
    fn odd_inputs_do_not_panic() {
        // unterminated string, non-bash syntax, empty, multibyte
        for cmd in ["echo 'oops", "if (= 1 2) { }", "", "echo héllo"] {
            for shell in [None, Some("fish"), Some("nu")] {
                assert_eq!(classify(cmd, shell).len(), cmd.len());
            }
        }
    }
}
