//! Names the program in the foreground in the title of the terminal the proxy runs in.
//!
//! A terminal that names the program running in it, in a tab or a window title, finds the proxy in
//! the foreground of its pty and shows `atuin` whatever the user runs. The proxy does what tmux's
//! `automatic-rename` does for a window name: whenever it follows the child's CWD, it looks at the
//! foreground process group of the pty its child runs on, and titles the terminal (`OSC 0`) with the
//! name of the group's leader.
//!
//! Once the child titles the terminal itself, with `OSC 0`, `1` or `2`, the proxy stops for good, as
//! tmux stops renaming a window once a program has named it. A shell that titles the terminal keeps
//! doing so around the commands it runs, and a program's title outlasts the program, as it would
//! without the proxy.
//!
//! The [`ForegroundUpdater`](crate::foreground::ForegroundUpdater) looks, and hands the [`Leader`]
//! it finds to the task that forwards the child's output. That task writes the title between two of
//! its writes, once [`TerminalTitle`], which follows the output, says the title would land outside
//! of any escape code or character.

use std::ffi::OsStr;

use atuin_common::os::unix::process;

use crate::foreground::Foreground;
use crate::parser::{Event, Output};

/// The leader of the process group in the foreground, as a look at the foreground found it.
pub struct Leader {
    /// The leader's name, fit for a title.
    name: String,
}

impl Leader {
    /// The leader of the process group in `foreground`'s foreground, if it can be named.
    ///
    /// Blocks: the name is read from the process table.
    pub fn of(foreground: &Foreground) -> Option<Self> {
        let name = Self::printable(&process::name(foreground.group()?)?)?;
        Some(Self { name })
    }

    /// `name` without its control characters, if anything is left.
    ///
    /// A process can call itself anything, and a control character would end the title early and
    /// leave the rest of the name for the terminal to run.
    fn printable(name: &OsStr) -> Option<String> {
        let name: String = name.to_string_lossy().chars().filter(|c| !c.is_control()).collect();
        (!name.is_empty()).then_some(name)
    }
}

/// What the proxy knows of the terminal's title, from following everything written to it.
pub struct TerminalTitle {
    /// Whether everything written to the terminal so far ends partway through an escape code or a
    /// character.
    mid_escape_code_or_char: bool,
    /// Whether the child has titled the terminal, which leaves the title to it for good.
    child_titled: bool,
    /// The title the proxy set last.
    shown: Option<String>,
}

impl TerminalTitle {
    pub const fn new() -> Self {
        Self {
            mid_escape_code_or_char: false,
            child_titled: false,
            shown: None,
        }
    }

    /// Follow the child's `output` on its way to the terminal.
    pub fn track(&mut self, output: &Output) {
        self.mid_escape_code_or_char = output.ends_mid_escape_code_or_char();
        self.child_titled |= output.chunks().any(|chunk| chunk.event == Event::Title);
    }

    /// The sequence that titles the terminal after `leader`, unless the child titles the terminal
    /// itself or the terminal shows that title already.
    ///
    /// The sequence must reach the terminal before any more of the child's output.
    pub fn retitle(&mut self, leader: &Leader) -> Option<Vec<u8>> {
        if self.child_titled
            || self.mid_escape_code_or_char
            || self.shown.as_ref() == Some(&leader.name)
        {
            return None;
        }
        let sequence = format!("\x1b]0;{}\x07", leader.name).into_bytes();
        self.shown = Some(leader.name.clone());
        Some(sequence)
    }
}

#[cfg(test)]
mod tests {
    use rstest::{fixture, rstest};

    use super::*;
    use crate::parser::Parser;

    /// The title of a terminal, with the parser of the output written to it.
    struct Terminal {
        title: TerminalTitle,
        output: Parser,
    }

    impl Terminal {
        /// Write `data` to the terminal, as if the child had printed it.
        fn print(&mut self, data: &[u8]) {
            let output = self.output.read(data.to_vec());
            self.title.track(&output);
        }

        /// What the proxy writes to the terminal once a look finds `name` in the foreground.
        fn retitle(&mut self, name: &str) -> Option<Vec<u8>> {
            self.title.retitle(&Leader {
                name: name.to_owned(),
            })
        }
    }

    #[fixture]
    fn terminal() -> Terminal {
        Terminal {
            title: TerminalTitle::new(),
            output: Parser::new(),
        }
    }

    fn titled(name: &str) -> Vec<u8> {
        format!("\x1b]0;{name}\x07").into_bytes()
    }

    #[rstest]
    #[case::plain("vim", Some("vim"))]
    #[case::escape("vim\x1b]0;pwned\x07", Some("vim]0;pwned"))]
    #[case::newline("vim\n", Some("vim"))]
    #[case::c1_terminator("vim\u{9c}", Some("vim"))]
    #[case::unicode("ñvim", Some("ñvim"))]
    #[case::only_controls("\x1b\x07", None)]
    #[case::empty("", None)]
    fn titles_carry_no_control_characters(#[case] name: &str, #[case] expected: Option<&str>) {
        assert_eq!(Leader::printable(OsStr::new(name)).as_deref(), expected);
    }

    #[rstest]
    fn names_each_new_leader_once(mut terminal: Terminal) {
        assert_eq!(terminal.retitle("zsh"), Some(titled("zsh")));
        assert_eq!(terminal.retitle("zsh"), None);
        assert_eq!(terminal.retitle("sleep"), Some(titled("sleep")));
    }

    #[rstest]
    fn stops_for_good_once_the_child_titles(mut terminal: Terminal) {
        assert_eq!(terminal.retitle("vim"), Some(titled("vim")));
        // What vim's `set title` sends.
        terminal.print(b"\x1b]2;file.txt - VIM\x07");
        assert_eq!(terminal.retitle("vim"), None);

        terminal.print(b"$ ");
        assert_eq!(terminal.retitle("zsh"), None);
    }

    #[rstest]
    fn titles_wait_for_an_escape_code_to_finish(mut terminal: Terminal) {
        terminal.print(b"\x1b[3");
        assert_eq!(terminal.retitle("zsh"), None);

        terminal.print(b"1m");
        assert_eq!(terminal.retitle("zsh"), Some(titled("zsh")));
    }
}
