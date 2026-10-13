//! Turning AI session capture on: `atuin config enable ai-sessions`, and the offer `atuin resume`
//! makes each time it runs while capture is off.

use std::io::{self, BufRead, BufReader, IsTerminal, Write};

use atuin_client::settings::Settings;
use colored::Colorize;
use eyre::Result;

use super::config::{EnableCmd, Feature};
use super::daemon;
use crate::i18n::fl;

/// Where the questions are asked and answered, and where the import's progress goes.
pub struct Terminal {
    /// `None` when nobody can answer, so every question is a no.
    input: Option<Box<dyn BufRead + Send>>,
    output: Box<dyn Write + Send>,
    /// Whether `output` is a terminal, where the import's progress is one line rewritten.
    live: bool,
    /// Keeps the terminal reading lines as typed while the questions are asked.
    #[cfg(unix)]
    _cooked: Option<Cooked>,
}

impl Terminal {
    /// Stdin and stdout, for `atuin config enable`.
    pub fn stdio() -> Self {
        // Asked only where it can be seen and answered.
        let live = io::stdout().is_terminal();
        let input =
            (live && io::stdin().is_terminal()).then(|| Box::new(BufReader::new(io::stdin())) as _);
        Self {
            input,
            output: Box::new(io::stdout()),
            live,
            #[cfg(unix)]
            _cooked: None,
        }
    }

    /// The controlling terminal, for `atuin resume`: the shell widget reads its stderr for the
    /// command, so nothing else may go there. `None` without one.
    #[cfg(unix)]
    pub fn tty() -> Option<Self> {
        let tty = std::fs::OpenOptions::new().read(true).write(true).open("/dev/tty").ok()?;
        Some(Self {
            input: Some(Box::new(BufReader::new(tty.try_clone().ok()?))),
            _cooked: Cooked::new(tty.try_clone().ok()?),
            output: Box::new(tty),
            live: true,
        })
    }

    #[cfg(not(unix))]
    pub fn tty() -> Option<Self> {
        None
    }

    /// Ask a yes-or-no question, yes by default, laid out as `atuin setup` asks. No once input
    /// has ended.
    fn ask(&mut self, question: &str, description: &str, note: Option<&str>) -> Result<bool> {
        let Some(input) = self.input.as_mut() else {
            return Ok(false);
        };

        let q = "[Y/n]".bold();
        writeln!(self.output, "> {}", question.bold().bright_blue())?;
        match note {
            Some(note) => write!(self.output, "  {description}\n  {note} {q} ")?,
            None => write!(self.output, "  {description} {q} ")?,
        }
        self.output.flush()?;

        let mut answer = String::new();
        // Ended (Ctrl-D) is a no; only Enter takes the default.
        if input.read_line(&mut answer)? == 0 {
            writeln!(self.output)?;
            return Ok(false);
        }
        Ok(matches!(answer.trim().to_lowercase().as_str(), "" | "y" | "yes"))
    }
}

/// The terminal set to read a line as typed (echoed, ended by Enter) for as long as this lives,
/// and back as it was after: the shell's line editor holds it raw while the resume widget runs.
#[cfg(unix)]
struct Cooked {
    tty: std::fs::File,
    was: rustix::termios::Termios,
}

#[cfg(unix)]
impl Cooked {
    fn new(tty: std::fs::File) -> Option<Self> {
        use rustix::termios::{InputModes, LocalModes, OptionalActions, OutputModes};

        let was = rustix::termios::tcgetattr(&tty).ok()?;
        let mut cooked = was.clone();
        cooked.local_modes |= LocalModes::ICANON | LocalModes::ECHO | LocalModes::ISIG;
        cooked.input_modes |= InputModes::ICRNL;
        cooked.output_modes |= OutputModes::OPOST | OutputModes::ONLCR;
        rustix::termios::tcsetattr(&tty, OptionalActions::Now, &cooked).ok()?;
        Some(Self { tty, was })
    }
}

#[cfg(unix)]
impl Drop for Cooked {
    fn drop(&mut self) {
        let now = rustix::termios::OptionalActions::Now;
        let _ = rustix::termios::tcsetattr(&self.tty, now, &self.was);
    }
}

/// Offer to turn AI sessions on, as `atuin resume` does while they're off. Returns the settings
/// with them on if the user said yes and they could be. Failures are reported on the terminal
/// rather than returned, so resume still goes on.
pub async fn offer(settings: &Settings, term: &mut Terminal) -> Option<Settings> {
    let enabled = async {
        if !term.ask(&fl!("ai-sessions-offer"), &fl!("ai-sessions-offer-description"), None)? {
            return Ok(None);
        }
        enable(settings, term).await.map(Some)
    }
    .await;

    match enabled {
        Ok(settings) => settings,
        Err(e) => {
            let _ =
                writeln!(term.output, "{}", fl!("ai-sessions-failed", error = format!("{e:#}")));
            None
        }
    }
}

/// Turn AI sessions on, with the daemon they need, and offer to import the sessions from before.
/// Returns the settings as they now are.
pub async fn enable(settings: &Settings, term: &mut Terminal) -> Result<Settings> {
    let config_file = Settings::get_config_path()?;
    let config_str = tokio::fs::read_to_string(&config_file).await?;
    let updated = EnableCmd {
        feature: Feature::AiSessions,
    }
    .get_updated_config(&config_str, settings.daemon.enabled)?;
    tokio::fs::write(&config_file, &updated).await?;

    let settings = Settings::new()?;

    // A daemon already running picks the change up itself; this starts one that isn't.
    if let Err(e) = daemon::ready_client(&settings).await {
        writeln!(term.output, "{}", fl!("ai-sessions-no-daemon", error = format!("{e:#}")))?;
        return Ok(settings);
    }
    writeln!(term.output, "{}", fl!("ai-sessions-enabled"))?;

    let note = settings.ai.capture_tools.then(|| fl!("ai-sessions-import-tools"));
    if term.ask(
        &fl!("ai-sessions-import"),
        &fl!("ai-sessions-import-description"),
        note.as_deref(),
    )? {
        // Capture is on whether or not this works, so it's no reason to say it isn't.
        if let Err(e) = atuin_ai::commands::import_all(&settings, &mut term.output, term.live).await
        {
            let error = format!("{e:#}");
            writeln!(term.output, "{}", fl!("ai-sessions-import-failed", error = error))?;
        }
    } else {
        writeln!(term.output, "{}", fl!("ai-sessions-import-later"))?;
    }

    Ok(settings)
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case::enter("\n", true)]
    #[case::yes("y\n", true)]
    #[case::no("n\n", false)]
    #[case::ended("", false)]
    fn an_answer_is_yes_by_default_and_no_once_input_ends(
        #[case] typed: &'static str,
        #[case] yes: bool,
    ) {
        let mut term = Terminal {
            input: Some(Box::new(typed.as_bytes())),
            output: Box::new(io::sink()),
            live: false,
            #[cfg(unix)]
            _cooked: None,
        };
        assert_eq!(term.ask("q", "d", None).unwrap(), yes);
    }
}
