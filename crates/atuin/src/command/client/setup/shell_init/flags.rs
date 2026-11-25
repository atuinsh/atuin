//! Reads and removes the keybinding flags of the `atuin init` calls in shell rc files.

use std::io;
use std::ops::Range;
use std::path::PathBuf;

use atuin_common::path::AbsPathBuf;

use super::encoding::Encoding;
use super::{GAP, InitCall, RcDirs, init_call};
use crate::command::client::init::{KeyFlags, Shell};
use crate::command::client::setup::plan::HOOKABLE;

/// The keybinding flags of `call`, an `atuin init` call that [`init_call`] found.
pub fn from_init(call: &str) -> KeyFlags {
    let mut flags = KeyFlags::default();
    for word in call.split(GAP) {
        match word {
            "--disable-ctrl-r" => flags.disable_ctrl_r = true,
            "--disable-up-arrow" => flags.disable_up_arrow = true,
            "--bind-ai-resume" => flags.bind_ai_resume = true,
            _ => {}
        }
    }
    flags
}

/// Whether [`strip_init_flags`] would take anything out of the given `atuin init` call.
pub fn any_to_strip(call: &str) -> bool {
    cuts(call).next().is_some()
}

/// What [`strip_init_flags`] did to the rc files.
#[derive(Debug, Default)]
pub struct Stripped {
    /// The rc files whose `atuin init` lines lost flags.
    pub changed: Vec<PathBuf>,
    /// The rc files that had flags to take out, but couldn't be written.
    pub failed: Vec<Unwritten>,
}

/// An rc file that [`strip_init_flags`] couldn't write.
#[derive(Debug)]
pub struct Unwritten {
    pub path: PathBuf,
    pub error: io::Error,
}

/// Take the keybinding flags out of every `atuin init` line in the shell rc files that setup
/// modifies.
///
/// A file that can't be read or decoded is left as it is. Detection skips that file too, so the
/// file's flags never reached the config. A file that can't be written is reported, and the rest
/// are still stripped.
pub fn strip_init_flags(dirs: &RcDirs) -> Stripped {
    let mut stripped = Stripped::default();
    for shell in HOOKABLE {
        for rc in dirs.rc_files(shell) {
            let target = AbsPathBuf::canonicalize(&rc).map_or_else(|_| rc.clone(), PathBuf::from);
            let Ok(read) = std::fs::read(&target) else {
                continue;
            };
            let Ok((encoding, text)) = Encoding::decode(&read) else {
                continue;
            };
            let Some(without) = text_without_flags(&text, shell) else {
                continue;
            };
            let bytes = encoding.encode(&without, true);
            match atuin_common::fs::replace(&target, &bytes, |now| now == read) {
                Ok(()) => stripped.changed.push(rc),
                // The directory didn't sync, but the file holds the new content.
                Err(error) if error.replaced() => stripped.changed.push(rc),
                Err(error) => stripped.failed.push(Unwritten {
                    path: rc,
                    error: io::Error::other(error),
                }),
            }
        }
    }
    stripped
}

/// `text`, the text of `shell`'s rc file, with the keybinding flags taken out of its `atuin init`
/// calls, each line keeping its own line ending. `None` when no line changes.
fn text_without_flags(text: &str, shell: Shell) -> Option<String> {
    let mut out: Option<String> = None;
    let mut copied = 0;
    for line in text.lines() {
        let Some(InitCall::Editable(call)) = init_call(line, shell) else {
            continue;
        };
        // `call` is a slice of `text`, so its address gives its offset.
        let call_start = call.as_ptr() as usize - text.as_ptr() as usize;
        for cut in cuts(call) {
            let out = out.get_or_insert_with(|| String::with_capacity(text.len()));
            out.push_str(&text[copied..call_start + cut.start]);
            copied = call_start + cut.end;
        }
    }
    out.map(|mut out| {
        out.push_str(&text[copied..]);
        out
    })
}

/// The byte ranges to cut from `call` to take its keybinding flags and `--disable-ai` out. Each
/// range holds a flag and the whitespace before it.
const fn cuts(call: &str) -> Cuts<'_> {
    Cuts { call, at: 0 }
}

/// The iterator that [`cuts`] returns.
struct Cuts<'a> {
    call: &'a str,
    /// Where the whitespace before the next word starts.
    at: usize,
}

impl Iterator for Cuts<'_> {
    type Item = Range<usize>;

    fn next(&mut self) -> Option<Range<usize>> {
        loop {
            let word = self.call[self.at..].trim_start_matches(GAP);
            if word.is_empty() {
                return None;
            }
            let gap = self.at;
            let len = word.find(GAP).unwrap_or(word.len());
            let start = self.call.len() - word.len();
            self.at = start + len;
            if matches!(
                &word[..len],
                "--disable-ctrl-r" | "--disable-up-arrow" | "--bind-ai-resume" | "--disable-ai"
            ) {
                return Some(gap..self.at);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use rstest::rstest;
    use tempfile::TempDir;

    use super::*;
    use crate::command::client::init::Shell;

    const ZSH: &str = "atuin init zsh";

    const FLAGGED_ZSH: &str = "eval \"$(atuin init zsh --disable-up-arrow --bind-ai-resume)\"";

    #[rstest]
    #[case(
        "atuin init zsh --disable-up-arrow",
        KeyFlags { disable_up_arrow: true, ..KeyFlags::default() }
    )]
    #[case(
        "atuin init fish --disable-ctrl-r --bind-ai-resume ",
        KeyFlags { disable_ctrl_r: true, bind_ai_resume: true, ..KeyFlags::default() }
    )]
    #[case("atuin init zsh --disable-up-arrows", KeyFlags::default())]
    #[case("atuin init zsh x--disable-ctrl-r", KeyFlags::default())]
    #[case::joined_to_a_quote(r#"atuin init zsh --disable-up-arrow"x""#, KeyFlags::default())]
    #[case::before_one_joined_to_a_quote(
        "atuin init zsh --disable-ctrl-r --disable-up-arrow'x'",
        KeyFlags { disable_ctrl_r: true, ..KeyFlags::default() }
    )]
    #[case::joined_to_a_backslash(r"atuin init zsh --disable-up-arrow\x", KeyFlags::default())]
    #[case::joined_to_a_backtick("atuin init fish --disable-up-arrow`x ", KeyFlags::default())]
    #[case::joined_to_a_hash("atuin init zsh --disable-up-arrow#x", KeyFlags::default())]
    #[case::after_a_wide_space("atuin init zsh --x\u{3000}--disable-ctrl-r", KeyFlags::default())]
    #[case::after_a_tab(
        "atuin init zsh\t--disable-ctrl-r",
        KeyFlags { disable_ctrl_r: true, ..KeyFlags::default() }
    )]
    fn flags_are_read_from_whole_words(#[case] call: &str, #[case] expected: KeyFlags) {
        assert_eq!(from_init(call), expected);
    }

    #[rstest]
    #[case(
        r#"eval "$(atuin init zsh --disable-up-arrow)""#,
        Shell::Zsh,
        Some(r#"eval "$(atuin init zsh)""#)
    )]
    #[case(
        "    atuin init fish --disable-up-arrow --bind-ai-resume | source",
        Shell::Fish,
        Some("    atuin init fish | source")
    )]
    #[case(
        r#"eval "$(atuin init bash --disable-ctrl-r --disable-ai)""#,
        Shell::Bash,
        Some(r#"eval "$(atuin init bash)""#)
    )]
    #[case(
        r#"eval "$(atuin init zsh --disable-ai)""#,
        Shell::Zsh,
        Some(r#"eval "$(atuin init zsh)""#)
    )]
    #[case(
        "atuin init powershell --disable-ctrl-r | Out-String | Invoke-Expression",
        Shell::PowerShell,
        Some("atuin init powershell | Out-String | Invoke-Expression")
    )]
    #[case(r#"eval "$(atuin init zsh)""#, Shell::Zsh, None)]
    #[case::in_a_trailing_comment(
        r#"eval "$(atuin init zsh)"  # was --disable-up-arrow"#,
        Shell::Zsh,
        None
    )]
    #[case::joined_to_a_quote(r#"atuin init bash --disable-up-arrow"xyz""#, Shell::Bash, None)]
    #[case::before_one_joined_to_a_quote(
        r#"atuin init bash --disable-ctrl-r --disable-up-arrow"xyz""#,
        Shell::Bash,
        Some(r#"atuin init bash --disable-up-arrow"xyz""#)
    )]
    #[case::crlf(
        "a\r\neval \"$(atuin init zsh --disable-ctrl-r)\"\r\nb\r\n",
        Shell::Zsh,
        Some("a\r\neval \"$(atuin init zsh)\"\r\nb\r\n")
    )]
    #[case::two_lines(
        "atuin init zsh --disable-ctrl-r\nexport A=1\natuin init zsh --disable-ai\n",
        Shell::Zsh,
        Some("atuin init zsh\nexport A=1\natuin init zsh\n")
    )]
    #[case::commented_out("# eval \"$(atuin init zsh --disable-ctrl-r)\"\n", Shell::Zsh, None)]
    #[case::other_lines("export A='--disable-ctrl-r'\n", Shell::Zsh, None)]
    #[case::bash_in_backticks(
        "eval \"`atuin init bash --disable-up-arrow`\"",
        Shell::Bash,
        Some("eval \"`atuin init bash`\"")
    )]
    #[case::fish_joined_to_a_backtick(
        "atuin init fish --disable-up-arrow` | source",
        Shell::Fish,
        None
    )]
    #[case::call_in_a_comment("export A=1  # atuin init zsh --disable-ctrl-r\n", Shell::Zsh, None)]
    #[case::after_a_wide_space("atuin init zsh --x\u{3000}--disable-ctrl-r", Shell::Zsh, None)]
    #[case::after_a_tab("atuin init zsh\t--disable-ctrl-r", Shell::Zsh, Some(ZSH))]
    fn text_loses_the_flags_of_its_init_calls(
        #[case] text: &str,
        #[case] shell: Shell,
        #[case] expected: Option<&str>,
    ) {
        assert_eq!(text_without_flags(text, shell).as_deref(), expected);
    }

    proptest! {
        #[rstest]
        fn every_flag_comes_out(
            present in prop::collection::vec(
                prop::sample::select(vec![
                    "--disable-ctrl-r",
                    "--disable-up-arrow",
                    "--bind-ai-resume",
                    "--disable-ai",
                ]),
                0..6,
            ),
            sep in prop::sample::select(vec![" ", "\t", "  "]),
        ) {
            let mut line = String::from(r#"eval "$(atuin init zsh"#);
            for flag in &present {
                line.push_str(sep);
                line.push_str(flag);
            }
            line.push_str(r#")""#);
            let result = text_without_flags(&line, Shell::Zsh).unwrap_or_else(|| line.clone());
            prop_assert_eq!(result, r#"eval "$(atuin init zsh)""#);
        }
    }

    #[rstest]
    fn stripping_flags_rewrites_each_copy() {
        let home = TempDir::new().unwrap();
        let dirs = RcDirs::under(home.path());
        let zshrc = dirs.rc_files(Shell::Zsh).remove(0);
        std::fs::write(&zshrc, format!("{FLAGGED_ZSH}\nexport A=1\n{FLAGGED_ZSH}\n")).unwrap();
        let fish = dirs.rc_files(Shell::Fish).remove(0);
        std::fs::create_dir_all(fish.parent().unwrap()).unwrap();
        let block = |flag: &str| {
            format!("if status is-interactive\n    atuin init fish{flag} | source\nend\n")
        };
        std::fs::write(&fish, block(" --disable-ctrl-r")).unwrap();

        assert_eq!(strip_init_flags(&dirs).changed, [zshrc.clone(), fish.clone()]);
        let init = "eval \"$(atuin init zsh)\"";
        assert_eq!(
            std::fs::read_to_string(&zshrc).unwrap(),
            format!("{init}\nexport A=1\n{init}\n")
        );
        assert_eq!(std::fs::read_to_string(&fish).unwrap(), block(""));
    }

    #[rstest]
    fn stripping_flags_skips_an_undecodable_file() {
        let home = TempDir::new().unwrap();
        let dirs = RcDirs::under(home.path());
        let bashrc = dirs.rc_files(Shell::Bash).remove(0);
        let invalid = b"\xff\xfe\xfa not utf-8\n";
        std::fs::write(&bashrc, invalid).unwrap();
        let zshrc = dirs.rc_files(Shell::Zsh).remove(0);
        std::fs::write(&zshrc, format!("{FLAGGED_ZSH}\n")).unwrap();

        assert_eq!(strip_init_flags(&dirs).changed, std::slice::from_ref(&zshrc));
        assert_eq!(std::fs::read_to_string(&zshrc).unwrap(), "eval \"$(atuin init zsh)\"\n");
        assert_eq!(std::fs::read(&bashrc).unwrap(), invalid);
    }

    #[rstest]
    fn stripping_flags_keeps_encoding_and_endings(
        #[values(Encoding::Utf8, Encoding::Utf16Le)] encoding: Encoding,
    ) {
        let home = TempDir::new().unwrap();
        let dirs = RcDirs::under(home.path());
        let profile = dirs.rc_files(Shell::PowerShell).remove(0);
        std::fs::create_dir_all(profile.parent().unwrap()).unwrap();
        let init = "atuin init powershell{} | Out-String | Invoke-Expression";
        let before = format!("Set-Alias g git\r\n{}\r\n", init.replace("{}", " --disable-ctrl-r"));
        std::fs::write(&profile, encoding.encode(&before, true)).unwrap();

        strip_init_flags(&dirs);
        let read = std::fs::read(&profile).unwrap();
        let after = format!("Set-Alias g git\r\n{}\r\n", init.replace("{}", ""));
        assert_eq!(Encoding::decode(&read).unwrap(), (encoding, after));
    }

    #[rstest]
    fn a_second_move_changes_nothing() {
        let home = TempDir::new().unwrap();
        let dirs = RcDirs::under(home.path());
        let zshrc = dirs.rc_files(Shell::Zsh).remove(0);
        std::fs::write(&zshrc, format!("{FLAGGED_ZSH}\n")).unwrap();

        assert_eq!(strip_init_flags(&dirs).changed, std::slice::from_ref(&zshrc));
        let moved = std::fs::read(&zshrc).unwrap();
        assert_eq!(strip_init_flags(&dirs).changed, Vec::<PathBuf>::new());
        assert_eq!(std::fs::read(&zshrc).unwrap(), moved);
    }
}
