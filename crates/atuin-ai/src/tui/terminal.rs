use crossterm::{
    cursor,
    terminal::{disable_raw_mode, enable_raw_mode},
};
use eyre::{Context, Result, bail};
use ratatui::{
    backend::CrosstermBackend,
    Terminal, TerminalOptions, Viewport,
};
use std::io::{stdout, IsTerminal, Stdout};

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

/// Guards terminal lifecycle, ensuring proper setup and cleanup.
///
/// # Lifecycle
/// - **Setup** (`new()`): Captures cursor position, enables raw mode, creates inline viewport
/// - **Cleanup** (`Drop`): Clears terminal, disables raw mode
///
/// # Safety Features
/// - Non-TTY detection: Returns error early if stdout is not a terminal
/// - Panic recovery: Works with `install_panic_hook()` to restore terminal after panic
/// - Drop-based cleanup: Ensures terminal is restored on normal exit
///
/// # Design Decisions
/// - Uses `Viewport::Inline(16)` to match existing inline.rs behavior
/// - Captures cursor position BEFORE raw mode to ensure accurate anchor point
/// - Ignores cleanup errors in Drop (best-effort restoration)
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
}

impl TerminalGuard {
    /// Create a new TerminalGuard, initializing the terminal for inline TUI mode.
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
    pub fn new() -> Result<Self> {
        // Non-TTY check: fail early if stdout is not a terminal
        if !stdout().is_terminal() {
            bail!("atuin-ai requires a terminal (TTY) but stdout is not a terminal. \
                   This typically happens when output is piped or redirected.");
        }

        // Capture cursor position BEFORE raw mode for accurate anchor
        let anchor_col = cursor::position()
            .map(|(x, _)| x)
            .unwrap_or(0);

        // Enable raw mode for keyboard input
        enable_raw_mode()
            .context("failed to enable raw mode")?;

        // Create terminal with inline viewport (matches existing inline.rs behavior)
        let backend = CrosstermBackend::new(stdout());
        let terminal = Terminal::with_options(
            backend,
            TerminalOptions {
                viewport: Viewport::Inline(16),
            },
        )
        .context("failed to create terminal with inline viewport")?;

        Ok(Self {
            terminal,
            anchor_col,
        })
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
/// 1. Clear terminal content
/// 2. Disable raw mode (restore normal terminal behavior)
///
/// # Error Handling
/// Errors are intentionally ignored during cleanup since:
/// - We're already exiting and can't meaningfully handle errors
/// - Best-effort restoration is better than panicking during Drop
/// - The panic hook provides a second layer of safety for abnormal exits
impl Drop for TerminalGuard {
    fn drop(&mut self) {
        // Clear terminal content - ignore errors (best-effort)
        let _ = self.terminal.clear();

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
