//! Terminal setup for the picker: raw mode, alternate screen (unless inline), mouse, bracketed
//! paste and keyboard enhancement, restored on drop, and on a panic before the panic is reported.
//! Mirrors the history search's `Stdout`.
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
use parking_lot::{Mutex, Once};
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

    /// Whether more input is waiting: the picker catches up with it before drawing again.
    pub fn behind(&self) -> bool {
        !self.rx.is_empty()
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

/// How the terminal was set up, while it is: what a panic has to undo.
static SETUP: Mutex<Option<(bool, bool)>> = Mutex::new(None);

fn take_setup() -> Option<(bool, bool)> {
    SETUP.lock().take()
}

/// Put the terminal back as [`TuiStdout::new`] found it, before a panic is reported, so the report
/// is readable and the shell isn't left in raw mode with the mouse captured. Once per process;
/// it does nothing while no picker is open.
fn restore_on_panic() {
    static HOOK: Once = Once::new();
    HOOK.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if let Some((inline_mode, no_mouse)) = take_setup()
                && let Ok(mut writer) = Writer::new()
            {
                restore(&mut writer, inline_mode, no_mouse);
            }
            previous(info);
        }));
    });
}

fn restore(writer: &mut Writer, inline_mode: bool, no_mouse: bool) {
    #[cfg(not(target_os = "windows"))]
    let _ = execute!(writer, PopKeyboardEnhancementFlags);
    if !inline_mode {
        let _ = execute!(writer, terminal::LeaveAlternateScreen);
    }
    if !no_mouse {
        let _ = execute!(writer, event::DisableMouseCapture);
    }
    let _ = execute!(writer, event::DisableBracketedPaste);
    let _ = terminal::disable_raw_mode();
}

pub struct TuiStdout {
    writer: Writer,
    inline_mode: bool,
    no_mouse: bool,
}

impl TuiStdout {
    pub fn new(inline_mode: bool, no_mouse: bool) -> io::Result<Self> {
        restore_on_panic();
        *SETUP.lock() = Some((inline_mode, no_mouse));
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
        // Already undone when a panic's hook got here first.
        if take_setup().is_some() {
            restore(&mut self.writer, self.inline_mode, self.no_mouse);
        }
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
