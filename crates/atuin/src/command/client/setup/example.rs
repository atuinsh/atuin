//! The examples the keys page plays under each key: a made-up terminal, where pressing the key
//! opens Atuin's search, picks a command and runs it, over and over.

use std::time::Duration;

use ratatui::style::{Color, Stylize};
use ratatui::text::{Line, Span};

use super::plan::Plan;

/// A key that can open Atuin's search.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Key {
    CtrlR,
    UpArrow,
}

impl Key {
    pub(super) const fn is_on(self, plan: &Plan) -> bool {
        match self {
            Self::CtrlR => plan.ctrl_r.0,
            Self::UpArrow => plan.up_arrow.0,
        }
    }

    const fn beats(self) -> &'static [Beat] {
        match self {
            Self::CtrlR => &CTRL_R,
            Self::UpArrow => &UP_ARROW,
        }
    }
}

/// What the example's terminal shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shows {
    /// The shell, once it has run the command, if there is one.
    Shell(Option<Ran>),
    /// Atuin's search with `query` typed, on the result `picked` up from the newest.
    Search {
        query: &'static str,
        picked: usize,
    },
}

/// A command the example runs, and what it prints.
type Ran = (&'static str, &'static str);

/// A beat of an example: the key pressed to get to it, if any, what it shows, and for how long.
struct Beat {
    key: Option<&'static str>,
    shows: Shows,
    lasts: Duration,
}

const fn beat(key: Option<&'static str>, shows: Shows, ms: u64) -> Beat {
    Beat {
        key,
        shows,
        lasts: Duration::from_millis(ms),
    }
}

const fn search(query: &'static str, picked: usize) -> Shows {
    Shows::Search { query, picked }
}

/// Ctrl-R: search for a command by what's in it.
const CTRL_R: [Beat; 7] = [
    beat(None, Shows::Shell(None), 1400),
    beat(Some("ctrl-r"), search("", 0), 700),
    beat(Some("g"), search("g", 0), 250),
    beat(Some("i"), search("gi", 0), 250),
    beat(Some("t"), search("git", 0), 900),
    beat(Some("↑"), search("git", 1), 900),
    beat(Some("enter"), Shows::Shell(Some(("git push", "Everything up-to-date"))), 2400),
];

/// The up arrow: search from the last command. It plays as long as [`CTRL_R`], so that it stays
/// as far behind as it starts.
const UP_ARROW: [Beat; 4] = [
    beat(None, Shows::Shell(None), 1400),
    beat(Some("↑"), search("", 0), 1600),
    beat(Some("↑"), search("", 1), 900),
    beat(
        Some("enter"),
        Shows::Shell(Some(("git status", "nothing to commit, working tree clean"))),
        2900,
    ),
];

/// How long a pressed key shows on the example's frame.
const KEY_SHOWN: Duration = Duration::from_millis(600);

/// The made-up history the search finds, oldest first: each command, how long it took, and when.
const HISTORY: [(&str, &str, &str); 7] = [
    ("git pull --rebase", "2.1s", "1h ago"),
    ("git checkout -b fix-build", "25ms", "32m ago"),
    ("git commit -m \"Fix the build\"", "18ms", "15m ago"),
    ("git push", "1.3s", "14m ago"),
    ("cargo test", "8.1s", "6m ago"),
    ("git status", "9ms", "2m ago"),
    ("cargo build --release", "41s", "1m ago"),
];

/// What ran in the made-up shell before the example began: the last three commands of [`HISTORY`].
const SESSION: [Ran; 3] = [
    ("cargo test", "test result: ok. 128 passed; 0 failed"),
    ("git status", "nothing to commit, working tree clean"),
    ("cargo build --release", "    Finished `release` profile in 41.2s"),
];

/// The rows inside the example's frame.
const ROWS: usize = 6;

/// The columns inside the example's frame, past its margin.
const COLUMNS: usize = 60;

/// The example of `key`, `at` into it, framed and indented by `indent` columns.
pub(super) fn example(key: Key, at: Duration, indent: usize) -> Vec<Line<'static>> {
    let Some((beat, into)) = beat_at(key.beats(), at) else {
        return Vec::new();
    };
    let pressed = beat.key.filter(|_| into < KEY_SHOWN);
    let indent = " ".repeat(indent);

    let title = " Example ";
    let top = vec![
        Span::raw(indent.clone()),
        "╭─".dim(),
        title.dim().italic(),
        "─".repeat(COLUMNS + 1 - title.len()).dim(),
        "╮".dim(),
    ];
    let mut lines = vec![Line::from(top)];
    lines.extend(screen(beat.shows).into_iter().map(|line| {
        let pad = " ".repeat(COLUMNS.saturating_sub(line.width()));
        let mut spans = vec![Span::raw(indent.clone()), "│ ".dim()];
        spans.extend(line.spans);
        spans.extend([Span::raw(pad), " │".dim()]);
        Line::from(spans)
    }));
    // The key being pressed, as on a screencast.
    let cap = pressed.map_or_else(String::new, |key| format!(" {key} "));
    let bottom = vec![
        Span::raw(indent),
        "╰".dim(),
        "─".repeat(COLUMNS - cap.chars().count()).dim(),
        Span::raw(cap).fg(Color::Black).bg(super::view::ACCENT).bold(),
        "──╯".dim(),
    ];
    lines.push(Line::from(bottom));
    lines
}

/// The beat of `beats` `at` into them, as they play over and over, and how far into it.
fn beat_at(beats: &[Beat], at: Duration) -> Option<(&Beat, Duration)> {
    let total: u128 = beats.iter().map(|beat| beat.lasts.as_millis()).sum();
    let at = at.as_millis().checked_rem(total)?;
    let at = Duration::from_millis(u64::try_from(at).ok()?);
    let starts = beats.iter().scan(Duration::ZERO, |start, beat| {
        let started = *start;
        *start += beat.lasts;
        Some((started, beat))
    });
    let (started, beat) = starts.take_while(|&(started, _)| started <= at).last()?;
    Some((beat, at.saturating_sub(started)))
}

/// What the terminal shows, [`ROWS`] lines of it.
fn screen(shows: Shows) -> Vec<Line<'static>> {
    match shows {
        Shows::Shell(ran) => {
            let ran = SESSION.iter().chain(ran.as_ref());
            let mut lines: Vec<_> = ran
                .flat_map(|&(command, said)| [prompt(command, false), Line::from(said)])
                .collect();
            lines.push(prompt("", true));
            lines.split_off(lines.len().saturating_sub(ROWS))
        }
        Shows::Search { query, picked } => {
            let found: Vec<_> = HISTORY
                .iter()
                .rev()
                .filter(|(command, ..)| command.contains(query))
                .take(ROWS - 1)
                .collect();
            // The newest is at the bottom, by the query, as Atuin draws it.
            let mut lines = vec![Line::default(); ROWS - 1 - found.len()];
            lines.extend(found.iter().enumerate().rev().map(|(i, &&(command, took, when))| {
                let line = format!("{took:>5} {when:>8}  {command}");
                if i == picked {
                    Line::from(vec![" > ".fg(super::view::ACCENT).bold(), Span::raw(line).bold()])
                } else {
                    Line::from(vec![Span::raw("   "), Span::raw(line)])
                }
            }));
            lines.push(Line::from(vec![
                "[ GLOBAL ] ".dim(),
                Span::raw(query),
                Span::raw(" ").reversed(),
            ]));
            lines
        }
    }
}

/// The made-up shell's prompt, with `command` typed, and the cursor after it if `waiting`.
fn prompt(command: &'static str, waiting: bool) -> Line<'static> {
    let mut spans = vec!["~/code".cyan(), Span::raw(" $ "), Span::raw(command)];
    if waiting {
        spans.push(Span::raw(" ").reversed());
    }
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    fn text(key: Key, at: Duration) -> Vec<String> {
        example(key, at, 0).iter().map(ToString::to_string).collect()
    }

    fn played(key: Key) -> Duration {
        key.beats().iter().map(|beat| beat.lasts).sum()
    }

    #[rstest]
    #[case::waits_in_the_shell(Key::CtrlR, 0, "    Finished `release` profile in 41.2s")]
    #[case::opens_atuin(Key::CtrlR, 1500, " >   41s   1m ago  cargo build --release")]
    #[case::shows_the_key(Key::CtrlR, 1500, "─ ctrl-r ──╯")]
    #[case::searches(Key::CtrlR, 3300, " >   9ms   2m ago  git status")]
    #[case::picks(Key::CtrlR, 4000, " >  1.3s  14m ago  git push")]
    #[case::runs_it(Key::CtrlR, 5500, "Everything up-to-date")]
    #[case::on_the_last_command(Key::UpArrow, 2000, " >   41s   1m ago  cargo build --release")]
    #[case::up_again(Key::UpArrow, 3500, " >   9ms   2m ago  git status")]
    #[case::runs_the_one_before(Key::UpArrow, 4500, "~/code $ git status")]
    fn an_example_opens_atuin_and_runs_what_it_picks(
        #[case] key: Key,
        #[case] ms: u64,
        #[case] expected: &str,
    ) {
        let shown = text(key, Duration::from_millis(ms));
        assert!(shown.iter().any(|line| line.contains(expected)), "{shown:#?}");
    }

    #[rstest]
    fn a_search_shows_what_matches_the_query() {
        let shown = text(Key::CtrlR, Duration::from_millis(3300));
        assert!(!shown.iter().any(|line| line.contains("cargo")), "{shown:#?}");
        assert!(shown.iter().any(|line| line.contains("[ GLOBAL ] git")), "{shown:#?}");
    }

    #[rstest]
    fn a_key_shows_only_as_it_is_pressed() {
        let shown = text(Key::CtrlR, Duration::from_millis(2050));
        assert!(shown.last().is_some_and(|line| !line.contains("ctrl-r")), "{shown:#?}");
    }

    #[rstest]
    fn an_example_plays_over_and_over(
        #[values(Key::CtrlR, Key::UpArrow)] key: Key,
        #[values(0, 1500, 3300)] ms: u64,
    ) {
        let at = Duration::from_millis(ms);
        assert_eq!(text(key, at), text(key, played(key) + at));
    }

    #[rstest]
    fn the_examples_play_as_long() {
        assert_eq!(played(Key::CtrlR), played(Key::UpArrow));
    }

    #[rstest]
    fn an_example_keeps_to_its_frame(#[values(Key::CtrlR, Key::UpArrow)] key: Key) {
        let mut moments = (0..played(key).as_millis()).step_by(50);
        let framed = |ms| {
            let lines = example(key, Duration::from_millis(u64::try_from(ms).unwrap()), 0);
            lines.len() == ROWS + 2 && lines.iter().all(|line| line.width() == COLUMNS + 4)
        };
        assert!(moments.all(framed));
    }
}
