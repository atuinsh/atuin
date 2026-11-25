//! The offer `atuin resume` makes, each time it runs while thread capture is off, to turn it on.

use std::io::{self, Write};

use atuin_client::settings::Settings;
use atuin_client::tui::cursor::Cursor;
use atuin_common::string::wrap;
use colored::Colorize;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use eyre::Result;
use toml_edit::Value;
use unicode_width::UnicodeWidthStr as _;

use super::config::update_config_file;
use super::daemon;
use crate::i18n::fl;

/// The widest that the questions are laid out, however wide the terminal is.
const MAX_COLUMNS: usize = 82;

/// How far in a question's description starts.
const INDENT: &str = "  ";

/// What follows a question's description, to say that Enter answers yes.
const PROMPT: &str = "[Y/n]";

/// Where the questions are asked and answered, and where the import's progress goes.
pub struct Terminal {
    /// The keys that the user presses. `None` when nobody can answer, so every question is a no.
    keys: Option<Box<dyn Iterator<Item = io::Result<KeyEvent>> + Send>>,
    output: Box<dyn Write + Send>,
    /// How wide `output` is.
    columns: usize,
    /// Whether `output` is a terminal, where the import's progress is one line rewritten.
    live: bool,
    /// Keeps the terminal reading keys as they're pressed, without echoing them.
    #[cfg(unix)]
    mode: Option<TtyMode>,
}

/// How the user answered a question.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Answer {
    Yes,
    No,
    /// Esc or Ctrl-C: stop, and resume nothing.
    Cancel,
}

impl Answer {
    /// The answer that a line typed and then entered gives: yes when it's empty.
    fn from_line(line: &str) -> Self {
        match line.trim().to_lowercase().as_str() {
            "" | "y" | "yes" => Self::Yes,
            _ => Self::No,
        }
    }
}

/// The line that is typed in answer to a question, edited a key at a time at its end.
struct Typed {
    line: Cursor,
}

/// What a key press did to a [`Typed`] line.
#[derive(Debug, PartialEq, Eq)]
enum Edit {
    Typed(char),
    Erased(char),
    Answered(Answer),
    Ignored,
}

impl Typed {
    fn new() -> Self {
        Self {
            line: Cursor::from(String::new()),
        }
    }

    fn press(&mut self, key: KeyEvent) -> Edit {
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            return match key.code {
                KeyCode::Char('c') => Edit::Answered(Answer::Cancel),
                // Ctrl-D ends input only on an empty line, as it does in a terminal.
                KeyCode::Char('d') if self.line.as_str().is_empty() => Edit::Answered(Answer::No),
                _ => Edit::Ignored,
            };
        }
        match key.code {
            KeyCode::Enter => Edit::Answered(Answer::from_line(self.line.as_str())),
            KeyCode::Esc => Edit::Answered(Answer::Cancel),
            KeyCode::Backspace => self.line.back().map_or(Edit::Ignored, Edit::Erased),
            KeyCode::Char(c) if !c.is_control() => {
                self.line.insert(c);
                Edit::Typed(c)
            }
            _ => Edit::Ignored,
        }
    }
}

/// Read a line from `keys`, echoed to `output`, until it answers. No once input has ended.
fn read_answer(
    keys: impl IntoIterator<Item = io::Result<KeyEvent>>,
    output: &mut impl Write,
) -> io::Result<Answer> {
    use unicode_width::UnicodeWidthChar as _;

    let mut typed = Typed::new();
    for key in keys {
        match typed.press(key?) {
            Edit::Typed(c) => write!(output, "{c}")?,
            Edit::Erased(c) => {
                for _ in 0..c.width().unwrap_or(0) {
                    write!(output, "\x08 \x08")?;
                }
            }
            Edit::Answered(answer) => return Ok(answer),
            Edit::Ignored => continue,
        }
        output.flush()?;
    }
    Ok(Answer::No)
}

impl Terminal {
    /// The controlling terminal, for `atuin resume`: the shell widget reads its stderr for the
    /// command, so nothing else may go there. `None` without one, or when it can't be set to
    /// hand over keys as they're pressed.
    #[cfg(unix)]
    pub fn tty() -> Option<Self> {
        let tty = std::fs::OpenOptions::new().read(true).write(true).open("/dev/tty").ok()?;
        let columns = crossterm::terminal::size().map_or(80, |(columns, _)| usize::from(columns));
        Some(Self {
            keys: Some(Box::new(key_presses())),
            mode: Some(TtyMode::keys(tty.try_clone().ok()?)?),
            output: Box::new(tty),
            columns,
            live: true,
        })
    }

    #[cfg(not(unix))]
    pub fn tty() -> Option<Self> {
        None
    }

    /// Ask a yes-or-no question, yes by default, laid out as `atuin setup` asks. No once input
    /// has ended.
    fn ask(&mut self, question: &str, description: &str, note: Option<&str>) -> Result<Answer> {
        let Some(keys) = self.keys.as_mut() else {
            return Ok(Answer::No);
        };

        writeln!(self.output, "> {}", question.bold().bright_blue())?;
        describe(&mut self.output, self.columns, description, note)?;
        self.output.flush()?;

        let answer = read_answer(keys, &mut self.output)?;
        writeln!(self.output)?;
        Ok(answer)
    }

    /// Put the terminal back as it was, after which every question is a no.
    fn release(&mut self) {
        self.keys = None;
        #[cfg(unix)]
        {
            self.mode = None;
        }
    }
}

/// Write `description` and `note` under a question, wrapped to fit in `terminal_columns`, and
/// the `[Y/n]` after.
fn describe(
    output: &mut impl Write,
    terminal_columns: usize,
    description: &str,
    note: Option<&str>,
) -> io::Result<()> {
    let columns = terminal_columns.min(MAX_COLUMNS).saturating_sub(INDENT.len());
    let lines =
        wrap(description, columns).chain(note.into_iter().flat_map(|note| wrap(note, columns)));

    let mut last = 0;
    for (i, line) in lines.enumerate() {
        let newline = if i == 0 {
            ""
        } else {
            "\n"
        };
        write!(output, "{newline}{INDENT}{line}")?;
        last = line.width();
    }

    // A space on each side of the prompt, the one after it being where the cursor waits.
    if last + PROMPT.len() + 2 > columns {
        write!(output, "\n{INDENT}")?;
    } else {
        write!(output, " ")?;
    }
    write!(output, "{} ", PROMPT.bold())
}

/// The keys pressed on the terminal, as they're pressed.
#[cfg(unix)]
fn key_presses() -> impl Iterator<Item = io::Result<KeyEvent>> + Send {
    use crossterm::event::{Event, KeyEventKind};

    std::iter::repeat_with(crossterm::event::read).filter_map(|event| match event {
        Ok(Event::Key(key)) if key.kind == KeyEventKind::Press => Some(Ok(key)),
        Ok(_) => None,
        Err(e) => Some(Err(e)),
    })
}

/// The terminal set to hand each key over as it's pressed for as long as this lives, and back as
/// it was after.
///
/// The terminal doesn't echo keys or turn Ctrl-C into a signal, so Ctrl-C reaches the question as
/// a key and this guard always gets to put the terminal back. A shell's line editor that runs
/// `atuin resume` as a widget expects the terminal as it left it.
#[cfg(unix)]
struct TtyMode {
    tty: std::fs::File,
    was: rustix::termios::Termios,
}

#[cfg(unix)]
impl TtyMode {
    fn keys(tty: std::fs::File) -> Option<Self> {
        use rustix::termios::{LocalModes, OptionalActions, OutputModes, SpecialCodeIndex};

        let was = rustix::termios::tcgetattr(&tty).ok()?;
        let mut keys = was.clone();
        keys.local_modes -= LocalModes::ICANON | LocalModes::ECHO | LocalModes::ISIG;
        keys.output_modes |= OutputModes::OPOST | OutputModes::ONLCR;
        keys.special_codes[SpecialCodeIndex::VMIN] = 1;
        keys.special_codes[SpecialCodeIndex::VTIME] = 0;
        rustix::termios::tcsetattr(&tty, OptionalActions::Now, &keys).ok()?;
        Some(Self { tty, was })
    }
}

#[cfg(unix)]
impl Drop for TtyMode {
    fn drop(&mut self) {
        let now = rustix::termios::OptionalActions::Now;
        let _ = rustix::termios::tcsetattr(&self.tty, now, &self.was);
    }
}

/// What came of offering to turn threads on.
pub enum Offer {
    /// Threads are on, and these are the settings as they now are.
    Enabled(Box<Settings>),
    /// Threads stay off, and resume goes on without them.
    Declined,
    /// The user asked to stop, so resume shouldn't go on. Threads are on when the user stopped at
    /// the import question.
    Cancelled,
}

/// Offer to turn threads on, as `atuin resume` does while they're off. Failures are reported on
/// the terminal rather than returned, so resume still goes on.
pub async fn offer(settings: &Settings, term: &mut Terminal) -> Offer {
    let enabled = async {
        match term.ask(&fl!("threads-offer"), &fl!("threads-offer-description"), None)? {
            Answer::Yes => enable(settings, term).await,
            Answer::No => Ok(Offer::Declined),
            Answer::Cancel => Ok(Offer::Cancelled),
        }
    }
    .await;

    enabled.unwrap_or_else(|e| {
        let _ = writeln!(term.output, "{}", fl!("threads-failed", error = format!("{e:#}")));
        Offer::Declined
    })
}

/// Turn threads on, with the daemon they need, and offer to import the threads from before.
/// Threads stay on when the user cancels at the import question.
async fn enable(settings: &Settings, term: &mut Terminal) -> Result<Offer> {
    let mut changes = vec![("threads.enabled", Value::from(true))];
    // A daemon on already may be managed externally (systemd, launchd), so only one this turns on
    // is left to start itself.
    if !settings.daemon.enabled {
        changes.push(("daemon.enabled", Value::from(true)));
        changes.push(("daemon.autostart", Value::from(true)));
    }
    update_config_file(changes).await?;

    let settings = Settings::new()?;

    // A daemon already running picks the change up itself; this starts one that isn't.
    if let Err(e) = daemon::ready_client(&settings).await {
        writeln!(term.output, "{}", fl!("threads-no-daemon", error = format!("{e:#}")))?;
        return Ok(Offer::Enabled(Box::new(settings)));
    }
    writeln!(term.output, "{}", fl!("threads-enabled"))?;

    let note = settings.threads.capture_tools.then(|| fl!("threads-import-tools"));
    let description = fl!("threads-import-description");
    match term.ask(&fl!("threads-import"), &description, note.as_deref())? {
        Answer::Yes => {
            // Ctrl-C stops the import as it stops any command, so the terminal goes back to how
            // the shell had it: a killed process can't put it back.
            term.release();
            // Capture is on whether or not this works, so it's no reason to say it isn't.
            let imported = atuin_ai::commands::import_all(&settings, &mut term.output, term.live);
            if let Err(e) = imported.await {
                let error = format!("{e:#}");
                writeln!(term.output, "{}", fl!("threads-import-failed", error = error))?;
            }
        }
        Answer::No => writeln!(term.output, "{}", fl!("threads-import-later"))?,
        Answer::Cancel => {
            writeln!(term.output, "{}", fl!("threads-import-later"))?;
            return Ok(Offer::Cancelled);
        }
    }

    Ok(Offer::Enabled(Box::new(settings)))
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    fn terminal(keys: Vec<KeyEvent>) -> Terminal {
        Terminal {
            keys: Some(Box::new(keys.into_iter().map(Ok))),
            output: Box::new(io::sink()),
            columns: 80,
            live: false,
            #[cfg(unix)]
            mode: None,
        }
    }

    fn key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    fn special(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn enter() -> KeyEvent {
        special(KeyCode::Enter)
    }

    #[rstest]
    #[case::enter(vec![enter()], Answer::Yes)]
    #[case::y(vec![key('y'), enter()], Answer::Yes)]
    #[case::yes_shouted(vec![key('Y'), key('E'), key('S'), enter()], Answer::Yes)]
    #[case::n(vec![key('n'), enter()], Answer::No)]
    #[case::anything_else(vec![key('x'), enter()], Answer::No)]
    #[case::erased_to_yes(vec![key('n'), special(KeyCode::Backspace), enter()], Answer::Yes)]
    #[case::other_keys_ignored(vec![special(KeyCode::Up), ctrl('y'), key('n'), enter()], Answer::No)]
    #[case::esc(vec![key('y'), special(KeyCode::Esc)], Answer::Cancel)]
    #[case::interrupt(vec![key('y'), ctrl('c')], Answer::Cancel)]
    #[case::ended(vec![ctrl('d')], Answer::No)]
    #[case::ended_after_typing(vec![key('y'), ctrl('d'), enter()], Answer::Yes)]
    #[case::input_ended(vec![key('y')], Answer::No)]
    fn a_typed_line_answers_when_entered(#[case] keys: Vec<KeyEvent>, #[case] answer: Answer) {
        let keys = keys.into_iter().map(Ok);
        assert_eq!(read_answer(keys, &mut io::sink()).unwrap(), answer);
    }

    #[rstest]
    #[case::typed(vec![key('n'), key('o')], "no")]
    #[case::erased(vec![key('n'), special(KeyCode::Backspace)], "n\x08 \x08")]
    #[case::erased_wide(vec![key('世'), special(KeyCode::Backspace)], "世\x08 \x08\x08 \x08")]
    #[case::erasing_nothing(vec![special(KeyCode::Backspace)], "")]
    fn the_line_is_echoed_as_typed(#[case] keys: Vec<KeyEvent>, #[case] shown: &str) {
        let mut output = Vec::new();
        read_answer(keys.into_iter().map(Ok), &mut output).unwrap();
        assert_eq!(String::from_utf8(output).unwrap(), shown);
    }

    #[rstest]
    #[case::input_ended(Some(vec![]))]
    #[case::nobody_to_answer(None)]
    fn without_an_answer_its_a_no(#[case] keys: Option<Vec<KeyEvent>>) {
        let mut term = terminal(vec![]);
        term.keys = keys.map(|keys| Box::new(keys.into_iter().map(Ok)) as _);
        assert_eq!(term.ask("q", "d", None).unwrap(), Answer::No);
    }

    /// The text that `describe` writes for a question `columns` wide.
    fn described(description: &str, note: Option<&str>, columns: usize) -> String {
        colored::control::set_override(false);
        let mut written = Vec::new();
        describe(&mut written, columns, description, note).unwrap();
        String::from_utf8(written).unwrap()
    }

    #[rstest]
    #[case::fits_with_the_prompt("one two", None, 80, "  one two [Y/n] ")]
    #[case::wrapped("one two three", None, 14, "  one two\n  three [Y/n] ")]
    #[case::prompt_on_a_line_of_its_own("one two", None, 10, "  one two\n  [Y/n] ")]
    #[case::note_on_a_line_of_its_own("one", Some("two"), 80, "  one\n  two [Y/n] ")]
    fn descriptions_wrap_to_the_terminal(
        #[case] description: &str,
        #[case] note: Option<&str>,
        #[case] columns: usize,
        #[case] expected: &str,
    ) {
        assert_eq!(described(description, note, columns), expected);
    }

    #[rstest]
    fn a_wide_terminal_still_wraps_at_the_maximum() {
        let description = fl!("threads-offer-description");
        let shown = described(&description, None, 500);
        assert!(shown.lines().count() > 1, "{shown}");
        for line in shown.lines() {
            assert!(line.width() <= MAX_COLUMNS, "{line:?}");
        }
    }
}
