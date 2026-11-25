use rstest::rstest;

use super::*;

const LINE: &str = "if [ -x /usr/bin/atuin ]; then eval \"$(/usr/bin/atuin desktop-capture login \
                    --shell sh 2>/dev/null)\"; fi";
const OTHER_LINE: &str =
    "if [ -x /opt/atuin ]; then eval \"$(/opt/atuin desktop-capture login --shell sh)\"; fi";

fn block() -> String {
    format!("{MARKER}\n{LINE}\n")
}

fn with(existing: &str) -> String {
    String::from_utf8(with_line(existing.as_bytes(), LINE, Syntax::Sh).text).unwrap()
}

fn without(existing: &str) -> String {
    String::from_utf8(without_line(existing.as_bytes())).unwrap()
}

const FEDORA_BASH_PROFILE: &str = "# .bash_profile\n\n# Get the aliases and functions\nif [ -f \
                                   ~/.bashrc ]; then\n\t. ~/.bashrc\nfi\n\n# User specific \
                                   environment and startup programs\n";
const UBUNTU_PROFILE: &str = "# ~/.profile: executed by the command interpreter for login \
                              shells.\n# This file is not read by bash(1), if ~/.bash_profile or \
                              ~/.bash_login\n# exists.\n\n# the default umask is set in \
                              /etc/profile; for setting the umask\n# for ssh logins, install and \
                              configure the libpam-umask package.\n#umask 022\n\n# if running \
                              bash\nif [ -n \"$BASH_VERSION\" ]; then\n    if [ -f \
                              \"$HOME/.bashrc\" ]; then\n\t. \"$HOME/.bashrc\"\n    fi\nfi\n";
const ARCH_BASHRC: &str = "#\n# ~/.bash_profile\n#\n\n[[ -f ~/.bashrc ]] && . ~/.bashrc\n";
const ARCH_EXEC: &str = "[ -z \"$WAYLAND_DISPLAY\" ] && [ \"$XDG_VTNR\" = 1 ] && exec sway\n";
const STARTX: &str =
    "# Start X on the first console.\nif [ \"$(tty)\" = /dev/tty1 ]; then\n  exec startx\nfi\n";
const CASE: &str =
    "case \"$(cat ~/session)\" in\n  sway) exec sway ;;\n  i3) exec startx ;;\nesac\n";

/// Without an `exec`, the block goes at the end after a blank line.
#[rstest]
#[case::empty("", "")]
#[case::ending_in_a_newline("export EDITOR=vi\n", "export EDITOR=vi\n\n")]
#[case::ending_in_a_blank_line("export EDITOR=vi\n\n", "export EDITOR=vi\n\n\n")]
#[case::fedora(FEDORA_BASH_PROFILE, &format!("{FEDORA_BASH_PROFILE}\n"))]
#[case::ubuntu(UBUNTU_PROFILE, &format!("{UBUNTU_PROFILE}\n"))]
#[case::exec_in_a_comment(
    "# exec sway\nexport A=1 # then exec sway\n",
    "# exec sway\nexport A=1 # then exec sway\n\n"
)]
#[case::exec_in_a_word(
    "execute\nexport exec_path=1\necho \"$exec_path\"\n",
    "execute\nexport exec_path=1\necho \"$exec_path\"\n\n"
)]
fn appends_the_block_without_an_exec(#[case] existing: &str, #[case] before: &str) {
    let with = with(existing);

    assert_eq!(with, format!("{before}{}", block()));
    assert_eq!(without(&with), existing);
    assert_eq!(self::with(&with), with, "a second run changes nothing");
}

/// A file without a final newline gets one, which stays after removal.
#[rstest]
fn a_file_without_a_final_newline_gets_one() {
    let with = with("export EDITOR=vi");
    assert_eq!(with, format!("export EDITOR=vi\n\n{}", block()));
    assert_eq!(without(&with), "export EDITOR=vi\n");
    assert_eq!(self::with(&with), with);
}

/// The block goes before the top-level statement that holds the first `exec`, and before the
/// comments directly above it, followed by a blank line.
#[rstest]
#[case::arch(ARCH_BASHRC, ARCH_EXEC)]
#[case::if_with_a_comment("export A=1\n\n", STARTX)]
#[case::case("export A=1\n", CASE)]
#[case::at_the_start("", "exec sway\n")]
#[case::after_a_shebang("#!/bin/sh\n", "exec sway\n")]
#[case::without_a_final_newline("export A=1\n", "exec sway")]
fn inserts_the_block_before_an_exec(#[case] before: &str, #[case] after: &str) {
    let existing = format!("{before}{after}");

    let with = with(&existing);

    assert_eq!(with, format!("{before}{}\n{after}", block()));
    assert_eq!(without(&with), existing);
    assert_eq!(self::with(&with), with, "a second run changes nothing");
}

/// Nushell's file always gets the block at the end. Setup can't tell where the statement of an
/// `exec` of Nushell starts, so the result says that the block comes after the `exec`.
#[rstest]
#[case::without_an_exec("$env.A = 1\n", Placement::BeforeAnyExec)]
#[case::exec_in_a_comment("# exec sway\n", Placement::BeforeAnyExec)]
#[case::exec("exec sway\n", Placement::AfterExec)]
#[case::exec_in_a_block("if (tty) == '/dev/tty1' { exec sway }\n", Placement::AfterExec)]
fn nu_appends_the_block(#[case] existing: &str, #[case] expected: Placement) {
    let placed = with_line(existing.as_bytes(), LINE, Syntax::Nu);

    assert_eq!(placed.placement, expected);
    let with = String::from_utf8(placed.text).unwrap();
    assert_eq!(with, format!("{existing}\n{}", block()));
    let again = with_line(with.as_bytes(), LINE, Syntax::Nu);
    assert_eq!(again.placement, expected, "a second run says the same");
    assert_eq!(String::from_utf8(again.text).unwrap(), with, "a second run changes nothing");
}

#[rstest]
#[case::present(format!("a\n\n{MARKER}\n{LINE}\n"), format!("a\n\n{MARKER}\n{LINE}\n"))]
#[case::another_path(format!("a\n{MARKER}\n{OTHER_LINE}\nb\n"), format!("a\nb\n\n{MARKER}\n{LINE}\n"))]
#[case::a_marker_at_the_end_without_a_newline(format!("a\n{MARKER}"), format!("a\n\n{MARKER}\n{LINE}\n"))]
#[case::a_marker_before_a_line_of_the_user(
    format!("{MARKER}\nexport EDITOR=vi\n"),
    format!("export EDITOR=vi\n\n{MARKER}\n{LINE}\n")
)]
#[case::a_call_that_the_user_commented_out(
    format!("{MARKER}\n# {OTHER_LINE}\n"),
    format!("# {OTHER_LINE}\n\n{MARKER}\n{LINE}\n")
)]
#[case::two_blocks(
    format!("{MARKER}\n{OTHER_LINE}\na\n{MARKER}\n{LINE}\nb\n"),
    format!("a\nb\n\n{MARKER}\n{LINE}\n")
)]
#[case::an_appended_block_after_an_exec(
    format!("{ARCH_BASHRC}{ARCH_EXEC}\n{MARKER}\n{OTHER_LINE}\n"),
    format!("{ARCH_BASHRC}{MARKER}\n{LINE}\n\n{ARCH_EXEC}")
)]
#[case::a_block_at_the_top(
    format!("# .bash_profile\n\n{MARKER}\n{LINE}\n\n. ~/.bashrc\n{ARCH_EXEC}"),
    format!("# .bash_profile\n\n. ~/.bashrc\n{MARKER}\n{LINE}\n\n{ARCH_EXEC}")
)]
#[case::a_block_at_the_top_without_an_exec(
    format!("# .profile\n\n{MARKER}\n{LINE}\n\nexport A=1\n"),
    format!("# .profile\n\nexport A=1\n\n{MARKER}\n{LINE}\n")
)]
fn replaces_earlier_blocks(#[case] existing: String, #[case] expected: String) {
    assert_eq!(with(&existing), expected);
    assert_eq!(with(&expected), expected, "a second run changes nothing");
}

#[rstest]
#[case::before_an_exec(format!("export A=1\n{MARKER}\n{LINE}\n\nexec sway\n"), "export A=1\nexec sway\n")]
#[case::before_an_exec_in_crlf(
    format!("export A=1\r\n{MARKER}\r\n{LINE}\r\n\r\nexec sway\r\n"),
    "export A=1\r\nexec sway\r\n"
)]
#[case::appended(format!("export EDITOR=vi\n\n{MARKER}\n{LINE}\n"), "export EDITOR=vi\n")]
#[case::in_the_middle(format!("a\n{MARKER}\n{LINE}\nb\n"), "a\nb\n")]
#[case::only_the_block(block(), "")]
#[case::none("a\n".to_owned(), "a\n")]
#[case::the_users_blank_line_at_the_end(format!("{MARKER}\n{LINE}\na\n\n"), "a\n\n")]
#[case::the_users_blank_line_after_a_block_in_the_middle(
    format!("a\n{MARKER}\n{LINE}\nb\n\n"),
    "a\nb\n\n"
)]
#[case::the_blank_line_that_setup_added(format!("a\n\n\n{MARKER}\n{LINE}\n"), "a\n\n")]
#[case::a_line_of_the_user(format!("{MARKER}\nexport EDITOR=vi\n"), "export EDITOR=vi\n")]
#[case::a_marker_alone_before_a_blank_line(format!("a\n{MARKER}\n\nb\n"), "a\n\nb\n")]
#[case::a_marker_alone_at_the_end(format!("a\n\n{MARKER}\n"), "a\n\n")]
#[case::a_call_that_the_user_commented_out(
    format!("{MARKER}\n  # {LINE}\n"),
    format!("  # {LINE}\n")
)]
fn removes_blocks(#[case] existing: String, #[case] expected: String) {
    assert_eq!(without(&existing), expected);
}

/// Bytes that are not UTF-8, such as Latin-1, stay as they are.
#[rstest]
fn keeps_text_in_another_encoding() {
    let existing = b"export NAME=Jos\xe9\n";

    let with = with_line(existing, LINE, Syntax::Sh).text;

    assert!(with.starts_with(b"export NAME=Jos\xe9\n\n"));
    assert_eq!(without_line(&with), existing);
}

/// When setup can't tell where the statement of the `exec` starts, the block goes at the end,
/// and the result says that the block comes after the `exec`.
#[rstest]
#[case::then_on_the_line_of_the_exec("coproc true\nif [ \"$XDG_VTNR\" = 1 ]\nthen exec sway\nfi\n")]
#[case::a_case_arm("coproc true\ncase \"$XDG_VTNR\" in\n  1) exec sway ;;\nesac\n")]
fn appends_the_block_after_an_exec_that_it_misreads(#[case] existing: &str) {
    let placed = with_line(existing.as_bytes(), LINE, Syntax::Sh);

    assert_eq!(placed.placement, Placement::AfterExec);
    let with = String::from_utf8(placed.text).unwrap();
    assert_eq!(with, format!("{existing}\n{}", block()));
    assert_eq!(without(&with), existing);
    assert_eq!(self::with(&with), with, "a second run changes nothing");
}

/// A block that the user moved above an `exec` that setup misreads stays where the user put
/// it.
#[rstest]
#[case::moved_above_the_exec(
    format!("coproc true\n{MARKER}\n{LINE}\n[ \"$XDG_VTNR\" = 1 ] && exec sway\n"),
    Placement::BeforeAnyExec
)]
#[case::left_at_the_end(
    format!("coproc true\nexec sway\n\n{MARKER}\n{LINE}\n"),
    Placement::AfterExec
)]
fn keeps_a_block_where_the_file_has_it(#[case] existing: String, #[case] expected: Placement) {
    let placed = with_line(existing.as_bytes(), LINE, Syntax::Sh);

    assert_eq!(placed.placement, expected);
    assert_eq!(String::from_utf8(placed.text).unwrap(), existing);
}

/// A block that calls another `atuin` moves to the end when setup misreads the `exec`.
#[rstest]
fn moves_an_outdated_block_after_an_exec_that_it_misreads() {
    let existing = format!("coproc true\n{MARKER}\n{OTHER_LINE}\nexec sway\n");

    let placed = with_line(existing.as_bytes(), LINE, Syntax::Sh);

    assert_eq!(placed.placement, Placement::AfterExec);
    let expected = format!("coproc true\nexec sway\n\n{}", block());
    assert_eq!(String::from_utf8(placed.text).unwrap(), expected);
}

#[cfg(unix)]
mod login;
