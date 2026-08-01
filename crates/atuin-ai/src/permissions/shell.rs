use atuin_common::shell::Command;

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
    subcommands: &[Command],
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
    use atuin_common::shell::ShellKind;
    use rstest::rstest;

    fn cmd<'a>(name: &'a str, full: &'a str) -> Command<'a> {
        Command { name, full }
    }

    // Parses that pin the exact extracted subcommands. `expected_fulls` is only
    // asserted when the original test did (`None` skips that check).
    #[rstest]
    #[case::simple_command(ShellKind::Bash, "ls -la /tmp", &["ls"], Some(&["ls -la /tmp"][..]))]
    #[case::pipeline(ShellKind::Bash, "cat file.txt | grep foo | wc -l", &["cat", "grep", "wc"], None)]
    #[case::command_chaining(
        ShellKind::Bash,
        "git add . && git commit -m 'hi'",
        &["git", "git"],
        Some(&["git add .", "git commit -m 'hi'"][..])
    )]
    #[case::semicolon_separated(ShellKind::Bash, "echo hello; echo world", &["echo", "echo"], None)]
    #[case::fish_simple_command(ShellKind::Fish, "ls -la /tmp", &["ls"], None)]
    #[case::fallback_double_ampersand_and_pipe_pipe(
        ShellKind::Unknown,
        "ls && cat foo || echo fail",
        &["ls", "cat", "echo"],
        Some(&["ls", "cat foo", "echo fail"][..])
    )]
    #[case::fallback_pipe_without_double(
        ShellKind::Unknown,
        "ls | grep foo",
        &["ls", "grep"],
        Some(&["ls", "grep foo"][..])
    )]
    #[case::subshell(ShellKind::Bash, "(cd /tmp && ls)", &["cd", "ls"], None)]
    #[case::variable_assignment_excluded(
        ShellKind::Bash,
        "FOO=bar ls -la /tmp",
        &["ls"],
        Some(&["ls -la /tmp"][..])
    )]
    #[case::variable_assignment_multiple(
        ShellKind::Bash,
        "A=1 B=2 git status",
        &["git"],
        Some(&["git status"][..])
    )]
    fn parse_exact(
        #[case] kind: ShellKind,
        #[case] code: &str,
        #[case] expected_names: &[&str],
        #[case] expected_fulls: Option<&[&str]>,
    ) {
        let cmds = kind.commands(code);
        let names: Vec<&str> = cmds.iter().map(|c| c.name).collect();
        assert_eq!(names, expected_names);
        if let Some(f) = expected_fulls {
            let fulls: Vec<&str> = cmds.iter().map(|c| c.full).collect();
            assert_eq!(fulls, f);
        }
    }

    // Parses that only require certain subcommands to be present (subset match).
    #[rstest]
    #[case::fallback_splits_correctly(ShellKind::Unknown, "ls && cat foo || echo fail", &["ls", "cat", "echo"])]
    #[case::fish_conditional(ShellKind::Fish, "git add .; and git commit -m hi", &["git"])]
    // `dash_is_parsed_as_posix`: only the posix grammar extracts the inner `git`
    // from `$(...)` -- the fallback parser cannot see past it -- so this proves
    // Dash routes to the posix parser rather than to the fallback path.
    #[case::dash_is_parsed_as_posix(ShellKind::Dash, "echo $(git rev-parse HEAD)", &["echo", "git"])]
    #[case::command_substitution(ShellKind::Bash, "echo $(git rev-parse HEAD)", &["echo", "git"])]
    #[case::backtick_substitution(ShellKind::Bash, "echo `date`", &["echo", "date"])]
    #[case::for_loop(ShellKind::Bash, "for f in *.txt; do cat $f; done", &["cat"])]
    #[case::if_statement(
        ShellKind::Bash,
        "if [ -f foo ]; then cat foo; else echo nope; fi",
        &["cat", "echo"]
    )]
    #[case::nested_substitution(
        ShellKind::Bash,
        "echo \"Result: $(git log --oneline | head -1)\"",
        &["echo", "git", "head"]
    )]
    #[case::fish_command_substitution(ShellKind::Fish, "echo (date)", &["echo", "date"])]
    fn parse_contains_names(
        #[case] kind: ShellKind,
        #[case] code: &str,
        #[case] expected: &[&str],
    ) {
        let cmds = kind.commands(code);
        let n: Vec<&str> = cmds.iter().map(|c| c.name).collect();
        for name in expected {
            assert!(n.contains(name), "should contain {name}: {n:?}");
        }
    }

    // Scope matching (`any_subcommand_matches`). Pure string matching -- no
    // parser involved -- so these run in every configuration. The `s0x` cases
    // were folded in from the previously separate `adversarial` scope tests.
    #[rstest]
    #[case::wildcard_star(&[("git", "git commit -m msg"), ("npm", "npm test")], true, "*", true)]
    #[case::git_commit_prefix(&[("git", "git commit -m msg"), ("npm", "npm test")], true, "git commit *", true)]
    #[case::git_push_prefix_no_match(&[("git", "git commit -m msg"), ("npm", "npm test")], true, "git push *", false)]
    #[case::git_push_bare_no_match(&[("git", "git commit -m msg"), ("npm", "npm test")], true, "git push", false)]
    #[case::npm_prefix(&[("git", "git commit -m msg"), ("npm", "npm test")], true, "npm *", true)]
    #[case::npm_test_bare(&[("git", "git commit -m msg"), ("npm", "npm test")], true, "npm test", true)]
    #[case::git_commit_bare_deny(&[("git", "git commit -m msg"), ("npm", "npm test")], true, "git commit", true)]
    #[case::git_commit_bare_allow(&[("git", "git commit -m msg"), ("npm", "npm test")], false, "git commit", false)]
    #[case::npm_test_exact_allow(&[("git", "git commit -m msg"), ("npm", "npm test")], false, "npm test", true)]
    #[case::ls_word_boundary(&[("ls", "ls -a"), ("lsof", "lsof -i :3000")], true, "ls *", true)]
    #[case::cat_word_boundary_no_match(&[("ls", "ls -a"), ("lsof", "lsof -i :3000")], true, "cat *", false)]
    #[case::lsof_word_boundary(&[("ls", "ls -a"), ("lsof", "lsof -i :3000")], true, "lsof *", true)]
    #[case::ls_glob_prefix(&[("ls", "ls -a"), ("lsof", "lsof -i :3000")], true, "ls*", true)]
    #[case::ls_exact(&[("ls", "ls")], true, "ls", true)]
    #[case::cat_exact_no_match(&[("ls", "ls")], true, "cat", false)]
    #[case::middle_wildcard_amend(&[("git", "git commit -m amend")], true, "git * amend", true)]
    #[case::middle_wildcard_commit_amend(&[("git", "git commit -m amend")], true, "git commit * amend", true)]
    #[case::middle_wildcard_push_amend_no_match(&[("git", "git commit -m amend")], true, "git push * amend", false)]
    #[case::middle_wildcard_zero_words(&[("git", "git commit")], true, "git * commit", true)]
    #[case::leading_wildcard_alpine(&[("docker", "docker run --rm alpine")], true, "* alpine", true)]
    #[case::leading_wildcard_ubuntu_no_match(&[("docker", "docker run --rm alpine")], true, "* ubuntu", false)]
    #[case::multiple_wildcards(&[("git", "git rebase -i HEAD~5")], true, "git * -i * HEAD~5", true)]
    #[case::multiple_wildcards_no_match(&[("git", "git rebase -i HEAD~5")], true, "git * -i * HEAD~10", false)]
    #[case::s01_empty_scope(&[("ls", "ls")], true, "", true)]
    #[case::s03_only_wildcard_space_star(&[("ls", "ls")], true, " *", true)]
    #[case::s04_glob_matches_empty(&[("ls", "ls")], true, "ls*", true)]
    #[case::s05_middle_wildcard_empty_match(&[("git", "git commit")], true, "git * commit", true)]
    #[case::s06_consecutive_wildcards(&[("git", "git commit")], true, "git ** commit", true)]
    #[case::s07_lower_wildcard_no_match(&[("LS", "LS -la")], true, "ls *", false)]
    #[case::s07_upper_wildcard(&[("LS", "LS -la")], true, "LS *", true)]
    #[case::s07_lower_bare_no_match(&[("LS", "LS -la")], true, "ls", false)]
    #[case::s07_upper_bare(&[("LS", "LS -la")], true, "LS", true)]
    #[case::s07_upper_bare_strict_no_match(&[("LS", "LS -la")], false, "LS", false)]
    #[case::s08_multi_word_exact_no_subcommand(&[("git", "git commit-amend")], true, "git commit", false)]
    fn scope_matching(
        #[case] commands: &[(&str, &str)],
        #[case] prefix_bare: bool,
        #[case] scope: &str,
        #[case] expected: bool,
    ) {
        let cmds: Vec<Command> = commands.iter().map(|(n, f)| cmd(n, f)).collect();
        assert_eq!(any_subcommand_matches(&cmds, prefix_bare, scope), expected);
    }
}

#[cfg(test)]
mod adversarial {
    use atuin_common::shell::ShellKind;
    use rstest::rstest;

    #[rstest]
    // Level 1: Basic compounds
    #[case::triple_chain(ShellKind::Bash, "a && b && c", &["a", "b", "c"])]
    #[case::or_chain(ShellKind::Bash, "a || b || c", &["a", "b", "c"])]
    #[case::mixed_chain(ShellKind::Bash, "a && b || c && d", &["a", "b", "c", "d"])]
    #[case::long_pipeline(
        ShellKind::Bash,
        "cat foo | grep bar | awk '{print $1}' | sort | uniq -c",
        &["cat", "grep", "awk", "sort", "uniq"]
    )]
    #[case::semicolons(ShellKind::Bash, "a; b; c; d", &["a", "b", "c", "d"])]
    // Command substitution
    #[case::nested_dollar(ShellKind::Bash, "echo $(basename $(dirname /foo/bar))", &["echo", "basename", "dirname"])]
    #[case::deeply_nested(ShellKind::Bash, "echo $(echo $(echo $(echo deep)))", &["echo", "echo", "echo", "echo"])]
    #[case::backtick_in_echo(ShellKind::Bash, "echo `hostname`", &["echo", "hostname"])]
    #[case::mixed_substitutions(ShellKind::Bash, "echo $(date) `uname`", &["echo", "date", "uname"])]
    // Subshells and groups
    #[case::subshell_chain(ShellKind::Bash, "(cd /tmp && ls -la)", &["cd", "ls"])]
    #[case::nested_subshells(ShellKind::Bash, "( (inner_cmd) )", &["inner_cmd"])]
    #[case::brace_group(ShellKind::Bash, "{ cd /tmp; ls; }", &["cd", "ls"])]
    // Variable assignments
    #[case::var_assignment_no_command(ShellKind::Bash, "FOO=bar", &[])]
    #[case::var_assignment_in_pipeline(ShellKind::Bash, "FOO=bar ls | BAZ=qux grep foo", &["ls", "grep"])]
    // Control flow
    #[case::if_then_else(ShellKind::Bash, "if [ -f foo ]; then cat foo; else echo missing; fi", &["cat", "echo"])]
    #[case::elif_chain(ShellKind::Bash, "if [ -f a ]; then cat a; elif [ -f b ]; then cat b; else echo none; fi", &["cat", "cat", "echo"])]
    #[case::for_loop(ShellKind::Bash, "for f in *.txt; do cat \"$f\"; done", &["cat"])]
    #[case::while_loop(ShellKind::Bash, "while read line; do echo \"$line\"; done < input.txt", &["echo", "read"])]
    #[case::case_statement(ShellKind::Bash, "case $x in foo) echo foo;; bar) echo bar;; esac", &["echo", "echo"])]
    // Redirection
    #[case::redirect_out(ShellKind::Bash, "ls > output.txt", &["ls"])]
    #[case::redirect_append(ShellKind::Bash, "ls >> output.txt 2>&1", &["ls"])]
    #[case::here_string(ShellKind::Bash, "grep foo <<< \"hello world\"", &["grep"])]
    #[case::redirect_in_pipeline(ShellKind::Bash, "cat < input.txt | sort | uniq", &["cat", "sort", "uniq"])]
    #[case::process_substitution(ShellKind::Bash, "diff <(sort a.txt) <(sort b.txt)", &["diff", "sort", "sort"])]
    // Functions
    #[case::function_def(ShellKind::Bash, "foo() { echo hello; }", &["echo"])]
    #[case::function_with_subshell(ShellKind::Bash, "build() { cargo build && cargo test; }", &["cargo", "cargo"])]
    // Edge cases
    #[case::empty_string(ShellKind::Bash, "", &[])]
    #[case::whitespace_only(ShellKind::Bash, "   \t  \n  ", &[])]
    #[case::single_command_no_args(ShellKind::Bash, "ls", &["ls"])]
    #[case::single_quotes(ShellKind::Bash, "echo 'hello world'", &["echo"])]
    #[case::double_quotes(ShellKind::Bash, "echo \"hello world\"", &["echo"])]
    #[case::escaped_spaces(ShellKind::Bash, "ls\\ -la", &["ls\\ -la"])]
    #[case::dollar_var(ShellKind::Bash, "echo $HOME/.bashrc", &["echo"])]
    #[case::background_job(ShellKind::Bash, "sleep 10 &", &["sleep"])]
    #[case::background_chain(ShellKind::Bash, "sleep 10 && echo done &", &["sleep", "echo"])]
    // Real-world
    #[case::docker_build_and_run(ShellKind::Bash, "docker build -t app . && docker run --rm app npm test", &["docker", "docker"])]
    #[case::git_rebase_interactive(ShellKind::Bash, "GIT_SEQUENCE_EDITOR=\"sed -i 's/pick/reword/'\" git rebase -i HEAD~5", &["git"])]
    #[case::find_with_exec(ShellKind::Bash, "find . -name '*.rs' -exec grep -l 'unsafe' {} +", &["find"])]
    #[case::curl_pipe_sh(ShellKind::Bash, "curl -sSL https://example.com/install.sh | bash", &["curl", "bash"])]
    #[case::xargs(ShellKind::Bash, "find . -name '*.tmp' | xargs rm -f", &["find", "xargs"])]
    #[case::npm_script_chain(ShellKind::Bash, "npm run build && npm run test && npm run lint", &["npm", "npm", "npm"])]
    #[case::make_with_redirect(ShellKind::Bash, "make -j$(nproc) 2>&1 | tee build.log", &["make", "nproc", "tee"])]
    #[case::sudo_chain(ShellKind::Bash, "sudo apt update && sudo apt upgrade -y", &["sudo", "sudo"])]
    #[case::here_doc_with_subcommand(ShellKind::Bash, "cat <<EOF\nhello $(whoami)\nEOF", &["cat", "whoami"])]
    #[case::eval_with_command(ShellKind::Bash, "eval \"echo hello\"", &["eval"])]
    #[case::exec_replace(ShellKind::Bash, "exec ls", &["exec"])]
    #[case::source_script(ShellKind::Bash, "source ~/.bashrc", &["source"])]
    // Fish
    #[case::fish_simple(ShellKind::Fish, "ls -la /tmp", &["ls"])]
    #[case::fish_pipe(ShellKind::Fish, "cat foo | grep bar | sort", &["cat", "grep", "sort"])]
    #[case::fish_and(ShellKind::Fish, "git add .; and git commit -m hi", &["git", "git"])]
    #[case::fish_or(ShellKind::Fish, "test -f foo; or echo missing", &["test", "echo"])]
    #[case::fish_not(ShellKind::Fish, "not test -f foo", &["test"])]
    #[case::fish_command_substitution(ShellKind::Fish, "echo (date)", &["echo", "date"])]
    #[case::fish_nested_substitution(ShellKind::Fish, "echo (basename (dirname /foo/bar))", &["echo", "basename", "dirname"])]
    #[case::fish_begin_end(ShellKind::Fish, "begin; ls; echo done; end", &["ls", "echo"])]
    #[case::fish_if_statement(ShellKind::Fish, "if test -f foo; cat foo; else; echo missing; end", &["cat", "echo", "test"])]
    #[case::fish_while_loop(ShellKind::Fish, "while true; echo tick; sleep 1; end", &["echo", "sleep", "true"])]
    #[case::fish_for_loop(ShellKind::Fish, "for f in *.txt; cat $f; end", &["cat"])]
    #[case::fish_switch(ShellKind::Fish, "switch $x; case foo; echo foo; case bar; echo bar; end", &["echo", "echo"])]
    #[case::fish_function_def(ShellKind::Fish, "function greet; echo hello $argv; end", &["echo"])]
    #[case::fish_redirect(ShellKind::Fish, "ls > output.txt", &["ls"])]
    #[case::fish_redirect_append(ShellKind::Fish, "ls >> output.txt", &["ls"])]
    #[case::fish_here_string(ShellKind::Fish, "grep foo <<< \"hello\"", &["grep"])]
    #[case::fish_curl_pipe(ShellKind::Fish, "curl -sSL https://example.com/install.sh | bash", &["curl", "bash"])]
    #[case::fish_double_ampersand(ShellKind::Fish, "git add . && git commit -m hi", &["git", "git"])]
    #[case::fish_double_pipe(ShellKind::Fish, "test -f foo || echo missing", &["test", "echo"])]
    #[case::fish_empty(ShellKind::Fish, "", &[])]
    #[case::fish_whitespace(ShellKind::Fish, "   ", &[])]
    fn extracts_subcommand_names(
        #[case] kind: ShellKind,
        #[case] code: &str,
        #[case] expected: &[&str],
    ) {
        let cmds = kind.commands(code);
        let mut got: Vec<&str> = cmds.iter().map(|c| c.name).collect();
        got.sort();
        let mut want: Vec<&str> = expected.to_vec();
        want.sort();
        assert_eq!(
            got, want,
            "{:?} parse of {:?}:\n  got:  {:?}\n  want: {:?}",
            kind, code, got, want
        );
    }

    #[rstest]
    #[case::single_var_assignment("FOO=bar ls", "ls", "ls")]
    #[case::multiple_var_assignments("A=1 B=2 C=3 git status", "git", "git status")]
    fn strips_var_assignments_keeping_full_command(
        #[case] code: &str,
        #[case] expected_name: &str,
        #[case] expected_full: &str,
    ) {
        let cmds = ShellKind::Bash.commands(code);
        let names: Vec<&str> = cmds.iter().map(|c| c.name).collect();
        assert_eq!(names, &[expected_name]);
        assert_eq!(cmds[0].full, expected_full);
    }
}
