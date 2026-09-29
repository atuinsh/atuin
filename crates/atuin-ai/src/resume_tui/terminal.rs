//! Terminal setup for the picker: raw mode, alternate screen (unless inline), mouse, bracketed
//! paste and keyboard enhancement, restored on drop. Mirrors the history search's `Stdout`.
//!
//! Also the picker's input: [`Events`] reads terminal events on a thread of its own.

use std::io::{self, IsTerminal, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crossterm::event::Event;
#[cfg(not(target_os = "windows"))]
use crossterm::event::{
    KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::{event, execute, terminal};
use tokio::sync::mpsc;

/// How long [`Events`] waits for input before letting go of crossterm's input lock, so a cursor
/// position query (which gives up after two seconds without it) always gets its turn.
const POLL: Duration = Duration::from_millis(100);

/// Terminal events, read on a thread of their own.
///
/// crossterm's `EventStream` waits for input holding crossterm's input lock for as long as none
/// comes. Anything that asks the terminal something meanwhile (ratatui asks where the cursor is
/// when it clears or resizes an inline viewport) can't take the lock, gives up after two seconds,
/// and fails with "The cursor position could not be read within a normal duration". The picker
/// often ends on a worker's answer (enter waits for the session's plan) while such a wait is
/// going on, so it failed on its way out. This waits in short polls instead, as the history
/// search does, letting go of the lock between them.
pub struct Events {
    rx: mpsc::UnboundedReceiver<io::Result<Event>>,
    stop: Arc<AtomicBool>,
}

impl Events {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        std::thread::spawn(move || {
            while !stopped.load(Ordering::Relaxed) && !tx.is_closed() {
                let event = match event::poll(POLL) {
                    Ok(false) => continue,
                    Ok(true) => event::read(),
                    Err(e) => Err(e),
                };
                let failed = event.is_err();
                if tx.send(event).is_err() || failed {
                    return;
                }
            }
        });
        Self { rx, stop }
    }

    /// The next event; `None` once reading has failed.
    pub async fn next(&mut self) -> Option<io::Result<Event>> {
        self.rx.recv().await
    }
}

impl Drop for Events {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

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
