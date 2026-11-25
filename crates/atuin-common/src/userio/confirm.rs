//! Yes-or-no questions on a terminal.

use std::fmt::Display;
use std::io::{self, BufRead, Write};

/// The user's answer to a question.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    Yes,
    No,
}

/// Writes `question` to `out` and reads the answer from `input`. "y" or "yes" accepts, "n" or
/// "no" declines, and an empty answer gives `default`. Any other answer or a Ctrl-D declines, so
/// that a mistyped answer changes nothing.
///
/// # Errors
///
/// Fails when `out` or `input` fails.
pub fn confirm(
    mut input: impl BufRead,
    out: &mut impl Write,
    question: impl Display,
    default: Answer,
) -> io::Result<Answer> {
    let choices = match default {
        Answer::Yes => "[Y/n]",
        Answer::No => "[y/N]",
    };
    write!(out, "{question} {choices} ")?;
    out.flush()?;
    let mut line = String::new();
    if input.read_line(&mut line)? == 0 {
        writeln!(out)?;
        return Ok(Answer::No);
    }
    let line = line.trim();
    Ok(if line.is_empty() {
        default
    } else if line.eq_ignore_ascii_case("y") || line.eq_ignore_ascii_case("yes") {
        Answer::Yes
    } else {
        Answer::No
    })
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case::yes("y\n", Answer::No, Answer::Yes)]
    #[case::yes_word("Yes\n", Answer::No, Answer::Yes)]
    #[case::empty("\n", Answer::No, Answer::No)]
    #[case::empty_by_default_yes("\n", Answer::Yes, Answer::Yes)]
    #[case::no_by_default_yes("n\n", Answer::Yes, Answer::No)]
    #[case::ended("", Answer::No, Answer::No)]
    #[case::ended_by_default_yes("", Answer::Yes, Answer::No)]
    #[case::other("nope\n", Answer::No, Answer::No)]
    #[case::other_by_default_yes("yep\n", Answer::Yes, Answer::No)]
    fn answers(#[case] input: &str, #[case] default: Answer, #[case] expected: Answer) {
        let mut out = Vec::new();

        let answer = confirm(input.as_bytes(), &mut out, "Go?", default).unwrap();

        assert_eq!(answer, expected);
    }

    #[rstest]
    #[case::default_no(Answer::No, "Go? [y/N] ")]
    #[case::default_yes(Answer::Yes, "Go? [Y/n] ")]
    fn the_question_shows_the_default(#[case] default: Answer, #[case] expected: &str) {
        let mut out = Vec::new();

        confirm(&b"\n"[..], &mut out, "Go?", default).unwrap();

        assert_eq!(String::from_utf8(out).unwrap(), expected);
    }
}
