//! Terminal setup for the picker: raw mode, alternate screen (unless inline), mouse, bracketed
//! paste and keyboard enhancement, restored on drop. Mirrors the history search's `Stdout`.

use std::io::{self, IsTerminal, Write};

#[cfg(not(target_os = "windows"))]
use crossterm::event::{
    KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::{event, execute, terminal};

/// Stdout, or `/dev/tty` when stdout is captured (`cmd=$(atuin ai resume)`).
enum Writer {
    Stdout(io::Stdout),
    #[cfg(unix)]
    Tty(std::fs::File),
}

impl Writer {
    fn new() -> io::Result<Self> {
        let stdout = io::stdout();
        if stdout.is_terminal() {
            return Ok(Self::Stdout(stdout));
        }
        #[cfg(unix)]
        {
            Ok(Self::Tty(std::fs::File::options().read(true).write(true).open("/dev/tty")?))
        }
        #[cfg(not(unix))]
        {
            Ok(Self::Stdout(stdout))
        }
    }
}

impl Write for Writer {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Self::Stdout(s) => s.write(buf),
            #[cfg(unix)]
            Self::Tty(f) => f.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Self::Stdout(s) => s.flush(),
            #[cfg(unix)]
            Self::Tty(f) => f.flush(),
        }
    }
}

pub struct TuiStdout {
    writer: Writer,
    inline_mode: bool,
    no_mouse: bool,
}

impl TuiStdout {
    pub fn new(inline_mode: bool, no_mouse: bool) -> io::Result<Self> {
        terminal::enable_raw_mode()?;
        let mut writer = Writer::new()?;
        if !inline_mode {
            execute!(writer, terminal::EnterAlternateScreen)?;
        }
        if !no_mouse {
            execute!(writer, event::EnableMouseCapture)?;
        }
        execute!(writer, event::EnableBracketedPaste)?;
        #[cfg(not(target_os = "windows"))]
        execute!(
            writer,
            PushKeyboardEnhancementFlags(
                KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                    | KeyboardEnhancementFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES
                    | KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS
            ),
        )?;
        Ok(Self {
            writer,
            inline_mode,
            no_mouse,
        })
    }
}

impl Drop for TuiStdout {
    fn drop(&mut self) {
        #[cfg(not(target_os = "windows"))]
        let _ = execute!(self.writer, PopKeyboardEnhancementFlags);
        if !self.inline_mode {
            let _ = execute!(self.writer, terminal::LeaveAlternateScreen);
        }
        if !self.no_mouse {
            let _ = execute!(self.writer, event::DisableMouseCapture);
        }
        let _ = execute!(self.writer, event::DisableBracketedPaste);
        let _ = terminal::disable_raw_mode();
    }
}

impl Write for TuiStdout {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.writer.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.writer.flush()
    }
}
