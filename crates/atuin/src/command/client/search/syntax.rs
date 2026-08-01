//! Shell syntax highlighting for the interactive history list, via
//! `atuin_common::shell`'s classifier. On platforms where tree-sitter's
//! bundled C doesn't build (see the note in Cargo.toml), commands are left
//! unhighlighted.

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
    use atuin_common::shell::{ShellKind, TokenKind};

    let kind = shell.map_or(ShellKind::Bash, |s| ShellKind::from_string(s.to_string()));
    let mut meanings = vec![Meaning::Base; cmd.len()];
    for t in kind.parser().classify(cmd) {
        let m = match t.kind {
            TokenKind::Command => Meaning::SyntaxCommand,
            TokenKind::Flag => Meaning::SyntaxFlag,
            TokenKind::String => Meaning::SyntaxString,
            TokenKind::Variable => Meaning::SyntaxVariable,
            TokenKind::Operator => Meaning::SyntaxOperator,
            TokenKind::Comment => Meaning::SyntaxComment,
        };
        if let Some(range) = meanings.get_mut(t.range) {
            range.fill(m);
        }
    }
    meanings
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub fn classify(cmd: &str, _shell: Option<&str>) -> Vec<Meaning> {
    vec![Meaning::Base; cmd.len()]
}

#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
mod tests {
    use super::{Meaning, classify};
    use rstest::rstest;

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
    #[case::zsh_uses_bash("ls -la", Some("zsh"), "ccafff")]
    #[case::nu_plain("ls -la", Some("nu"), "ccaaaa")]
    #[case::powershell_plain("ls -la", Some("powershell"), "ccaaaa")]
    fn classify_renders(#[case] cmd: &str, #[case] shell: Option<&str>, #[case] expected: &str) {
        assert_eq!(render_shell(cmd, shell), expected);
    }

    #[rstest]
    fn odd_inputs_do_not_panic(
        // unterminated string, non-bash syntax, empty, multibyte
        #[values("echo 'oops", "if (= 1 2) { }", "", "echo héllo")] cmd: &str,
        #[values(None, Some("fish"), Some("nu"))] shell: Option<&str>,
    ) {
        assert_eq!(classify(cmd, shell).len(), cmd.len());
    }
}
