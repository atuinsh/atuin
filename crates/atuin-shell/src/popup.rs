//! Popup lifecycle management.
//!
//! When a popup request is received, this module:
//!
//! 1. Spawns the requested command in a fresh PTY
//! 2. Redirects the shared stdin writer to the popup PTY
//! 3. Switches the real terminal to the alternate screen
//! 4. Forwards popup PTY output → real stdout (blocking the caller)
//! 5. On exit, leaves alternate screen, restores stdin, reads result
//!
//! The caller is responsible for providing the shared stdin writer (a
//! `Mutex<Box<dyn Write + Send>>`) that the stdin-reader thread writes to.
//! During the popup, this writer is temporarily swapped to the popup PTY.
//!
//! # Environment provided to popup commands
//!
//! | Variable              | Value                                    |
//! |-----------------------|------------------------------------------|
//! | `ATUIN_POPUP`         | `1` — signals we're in a popup context   |
//! | `ATUIN_POPUP_RESULT`  | Path to write result data to             |

use std::io::{Read, Write};
use std::sync::Mutex;

use crossterm::execute;
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use portable_pty::{CommandBuilder, PtySize, native_pty_system};

/// Outcome of a popup invocation.
pub struct PopupResult {
    /// The exit code of the popup process.
    pub exit_code: u32,
    /// Content written to the result file, if any.
    pub output: Option<String>,
}

/// Run a command in a popup over the real terminal.
///
/// `command` is the shell command string to execute (passed to `sh -c`).
///
/// `stdin_writer` is the shared writer that the stdin-reader thread sends
/// keystrokes to.  During the popup it is temporarily replaced with the
/// popup PTY's writer, then restored on exit (even on error).
///
/// This function blocks the calling thread for the lifetime of the popup,
/// forwarding the popup PTY's output to real stdout.
pub fn run(
    command: &str,
    stdin_writer: &Mutex<Box<dyn Write + Send>>,
) -> eyre::Result<PopupResult> {
    let (cols, rows) = terminal::size()?;

    // Result file — the popup command writes its output here.
    let result_path = std::env::temp_dir().join(format!("atuin-popup-{}", std::process::id()));

    // -- spawn popup in a new PTY --

    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| eyre::eyre!("{e:#}"))?;

    let mut cmd = CommandBuilder::new("sh");
    cmd.arg("-c");
    cmd.arg(command);
    cmd.env("ATUIN_POPUP", "1");
    cmd.env("ATUIN_POPUP_RESULT", &result_path);
    if let Ok(cwd) = std::env::current_dir() {
        cmd.cwd(cwd);
    }

    let mut child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|e| eyre::eyre!("{e:#}"))?;

    drop(pair.slave);

    let mut popup_reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| eyre::eyre!("{e:#}"))?;
    let popup_writer = pair
        .master
        .take_writer()
        .map_err(|e| eyre::eyre!("{e:#}"))?;

    // Handle terminal resize for the popup PTY.
    let master = pair.master;
    let _resize_thread = {
        use signal_hook::consts::SIGWINCH;
        use signal_hook::iterator::Signals;

        let mut signals = Signals::new([SIGWINCH])?;

        std::thread::spawn(move || {
            for _ in signals.forever() {
                if let Ok((cols, rows)) = terminal::size() {
                    let _ = master.resize(PtySize {
                        rows,
                        cols,
                        pixel_width: 0,
                        pixel_height: 0,
                    });
                }
            }
        })
    };

    // -- redirect stdin to popup PTY --
    //
    // Swap the writer under the lock.  The stdin-reader thread will
    // immediately start sending keystrokes to the popup PTY.
    let original_writer = {
        let mut w = stdin_writer.lock().unwrap();
        std::mem::replace(&mut *w, popup_writer)
    };

    // -- switch to alternate screen --

    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen)?;

    // -- forward popup PTY → real stdout (blocking) --

    let mut buf = [0u8; 8192];
    loop {
        match popup_reader.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if stdout.write_all(&buf[..n]).is_err() {
                    break;
                }
                let _ = stdout.flush();
            }
        }
    }

    // Ensure the process has fully exited.
    let status = child.wait()?;

    // -- restore: alternate screen, then stdin --

    execute!(stdout, LeaveAlternateScreen)?;

    {
        let mut w = stdin_writer.lock().unwrap();
        *w = original_writer;
    }

    // -- read result file if present --

    let output = std::fs::read_to_string(&result_path).ok().and_then(|s| {
        let trimmed = s.trim().to_string();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        }
    });
    let _ = std::fs::remove_file(&result_path);

    Ok(PopupResult {
        exit_code: status.exit_code(),
        output,
    })
}

/// Format a popup APC response to inject into the shell's PTY.
///
/// The shell integration can parse this to retrieve the popup's result.
///
/// Wire format: `ESC _ atuin;result <data> ESC \`
pub fn encode_result(data: &str) -> Vec<u8> {
    let mut buf = Vec::with_capacity(16 + data.len());
    buf.extend_from_slice(b"\x1b_atuin;result ");
    buf.extend_from_slice(data.as_bytes());
    buf.extend_from_slice(b"\x1b\\");
    buf
}
