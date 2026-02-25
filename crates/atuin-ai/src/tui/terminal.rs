use crossterm::{
    cursor,
    terminal::{disable_raw_mode, enable_raw_mode},
};
use eyre::{Context, Result, bail};
use ratatui::{Terminal, TerminalOptions, Viewport, backend::CrosstermBackend};
use std::io::{IsTerminal, Stdout, stdout};

/// Install a panic hook that ensures the terminal is restored to a usable state
/// even if the application panics.
///
/// This must be called before creating the TerminalGuard to ensure proper cleanup
/// during panics. The hook will:
/// 1. Disable raw mode (restoring normal terminal behavior)
/// 2. Call the original panic hook to display panic information
///
/// # Implementation Note
/// This satisfies TUI-07: Terminal remains usable after panic by ensuring
/// disable_raw_mode() is called before the panic message is displayed.
pub fn install_panic_hook() {
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        // Attempt to restore terminal - ignore errors since we're already panicking
        let _ = disable_raw_mode();
        // Call original hook to display panic with backtrace
        original_hook(panic_info);
    }));
}

/// Minimum viewport height
const MIN_VIEWPORT_HEIGHT: u16 = 10;

/// Margin to leave below viewport for shell prompt
const VIEWPORT_BOTTOM_MARGIN: u16 = 2;

/// Guards terminal lifecycle, ensuring proper setup and cleanup.
///
/// # Lifecycle
/// - **Setup** (`new()`): Captures cursor position, enables raw mode, creates inline viewport
/// - **Cleanup** (`Drop`): Clears terminal, disables raw mode
///
/// # Dynamic Viewport Sizing
/// The viewport starts at 15 lines (enough for simple commands) and grows
/// dynamically when content requires more space. Use `ensure_height()` before
/// rendering to grow the viewport if needed.
///
/// # Safety Features
/// - Non-TTY detection: Returns error early if stdout is not a terminal
/// - Panic recovery: Works with `install_panic_hook()` to restore terminal after panic
/// - Drop-based cleanup: Ensures terminal is restored on normal exit
///
/// # Example
/// ```no_run
/// use atuin_ai::tui::{install_panic_hook, TerminalGuard};
///
/// install_panic_hook(); // Once at program start
/// let mut guard = TerminalGuard::new()?;
/// let terminal = guard.terminal();
/// // ... use terminal ...
/// // Drop automatically cleans up
/// # Ok::<(), eyre::Report>(())
/// ```
pub struct TerminalGuard {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    anchor_col: u16,
    keep_output: bool,
    viewport_height: u16,
}

impl TerminalGuard {
    /// Create a new TerminalGuard, initializing the terminal for inline TUI mode.
    ///
    /// # Arguments
    /// * `keep_output` - If true, preserve TUI output on exit; if false, clear it
    ///
    /// # Process
    /// 1. Check if stdout is a terminal (non-TTY detection)
    /// 2. Capture cursor position for inline rendering anchor
    /// 3. Enable raw mode for keyboard input
    /// 4. Create terminal with inline viewport
    ///
    /// # Errors
    /// - Returns error if stdout is not a terminal (e.g., piped or redirected)
    /// - Returns error if terminal initialization fails
    ///
    /// # Implementation Note
    /// Cursor position is captured BEFORE enabling raw mode because some terminals
    /// may report position differently after raw mode is enabled.
    pub fn new(keep_output: bool) -> Result<Self> {
        // Non-TTY check: fail early if stdout is not a terminal
        if !stdout().is_terminal() {
            bail!(
                "atuin-ai requires a terminal (TTY) but stdout is not a terminal. \
                   This typically happens when output is piped or redirected."
            );
        }

        // Get terminal size and calculate viewport height
        let (_, term_height) = crossterm::terminal::size().unwrap_or((80, 24));
        let viewport_height = term_height
            .saturating_sub(VIEWPORT_BOTTOM_MARGIN)
            .max(MIN_VIEWPORT_HEIGHT);

        // Capture cursor position BEFORE raw mode for accurate anchor
        let anchor_col = cursor::position().map(|(x, _)| x).unwrap_or(0);

        // Enable raw mode for keyboard input
        enable_raw_mode().context("failed to enable raw mode")?;

        // Create terminal with fixed viewport based on terminal size
        let backend = CrosstermBackend::new(stdout());
        let terminal = Terminal::with_options(
            backend,
            TerminalOptions {
                viewport: Viewport::Inline(viewport_height),
            },
        )
        .context("failed to create terminal with inline viewport")?;

        Ok(Self {
            terminal,
            anchor_col,
            keep_output,
            viewport_height,
        })
    }

    /// Returns the current viewport height.
    ///
    /// The viewport is fixed at creation time based on terminal size.
    /// Content that exceeds this height will be scrolled automatically.
    ///
    /// The `_needed` parameter is kept for API compatibility but ignored -
    /// we no longer attempt to resize the viewport dynamically since that
    /// operation can fail unpredictably with inline viewports.
    pub fn ensure_height(&mut self, _needed: u16) -> Result<u16> {
        Ok(self.viewport_height)
    }

    /// Get the current viewport height.
    pub fn viewport_height(&self) -> u16 {
        self.viewport_height
    }

    /// Get mutable reference to the underlying terminal.
    ///
    /// Use this to perform rendering operations.
    pub fn terminal(&mut self) -> &mut Terminal<CrosstermBackend<Stdout>> {
        &mut self.terminal
    }

    /// Get the anchor column where the inline UI should be positioned.
    ///
    /// This is the column position where the cursor was located when
    /// the terminal was initialized.
    pub fn anchor_col(&self) -> u16 {
        self.anchor_col
    }
}

/// Cleanup terminal state when TerminalGuard is dropped.
///
/// This implements TUI-08: Terminal restores correctly after normal exit.
///
/// # Cleanup Process
/// 1. Conditionally clear terminal content (based on keep_output flag)
/// 2. Disable raw mode (restore normal terminal behavior)
///
/// # Error Handling
/// Errors are intentionally ignored during cleanup since:
/// - We're already exiting and can't meaningfully handle errors
/// - Best-effort restoration is better than panicking during Drop
/// - The panic hook provides a second layer of safety for abnormal exits
impl Drop for TerminalGuard {
    fn drop(&mut self) {
        // Clear terminal content only if keep_output is false - ignore errors (best-effort)
        if !self.keep_output {
            let _ = self.terminal.clear();
        }

        // Disable raw mode to restore normal terminal behavior - ignore errors
        let _ = disable_raw_mode();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_panic_hook_installation() {
        // Test that panic hook can be installed without error
        install_panic_hook();
        // Installing again should work (replaces previous hook)
        install_panic_hook();
    }

    // Note: Cannot easily test TerminalGuard::new() in CI since it requires a TTY.
    // Manual testing required for:
    // 1. Non-TTY detection: echo "" | cargo run -p atuin-ai -- inline
    // 2. Drop cleanup: Run inline command, press Esc, verify terminal is normal
    // 3. Panic recovery: Add panic!("test") after TerminalGuard::new(), verify terminal is usable
}
