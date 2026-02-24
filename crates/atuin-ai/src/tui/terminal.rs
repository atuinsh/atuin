use std::io::{IsTerminal, Stdout, stdout};

use crossterm::{
    cursor,
    terminal::{disable_raw_mode, enable_raw_mode},
};
use eyre::{Context, Result, bail};
use ratatui::{backend::Backend, backend::CrosstermBackend, text::Line};

use super::custom_terminal;

/// Install a panic hook that ensures the terminal is restored to a usable state
/// even if the application panics.
pub fn install_panic_hook() {
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        let _ = disable_raw_mode();
        original_hook(panic_info);
    }));
}

/// Minimum viewport height.
const MIN_VIEWPORT_HEIGHT: u16 = 10;

/// Margin to leave below viewport for shell prompt.
const VIEWPORT_BOTTOM_MARGIN: u16 = 2;

pub struct TerminalGuard {
    terminal: custom_terminal::Terminal<CrosstermBackend<Stdout>>,
    anchor_col: u16,
    keep_output: bool,
    viewport_height: u16,
    pending_history_lines: Vec<Line<'static>>,
}

impl TerminalGuard {
    /// Create a new TerminalGuard, initializing terminal state for inline TUI mode.
    pub fn new(keep_output: bool) -> Result<Self> {
        if !stdout().is_terminal() {
            bail!(
                "atuin-ai requires a terminal (TTY) but stdout is not a terminal. \
                   This typically happens when output is piped or redirected."
            );
        }

        // Capture cursor position before raw mode for stable anchor placement.
        let (anchor_col, anchor_row) = cursor::position().unwrap_or((0, 0));

        enable_raw_mode().context("failed to enable raw mode")?;

        let backend = CrosstermBackend::new(stdout());
        let mut terminal = custom_terminal::Terminal::with_options(backend)
            .context("failed to create custom inline terminal")?;

        let size = terminal
            .size()
            .unwrap_or_else(|_| ratatui::layout::Size::new(80, 24));
        let viewport_height = initial_viewport_height(size.height);
        let viewport_area = ratatui::layout::Rect::new(0, anchor_row, size.width, viewport_height);
        terminal.set_viewport_area(viewport_area);

        Ok(Self {
            terminal,
            anchor_col,
            keep_output,
            viewport_height,
            pending_history_lines: Vec::new(),
        })
    }

    /// Ensure viewport is tall enough for the requested content and return chosen height.
    pub fn ensure_height(&mut self, needed: u16) -> Result<u16> {
        let terminal_height = self
            .terminal
            .size()
            .context("failed to query terminal size")?
            .height;

        let max_height = terminal_height
            .saturating_sub(VIEWPORT_BOTTOM_MARGIN)
            .max(1);
        let min_height = MIN_VIEWPORT_HEIGHT.min(max_height);
        self.viewport_height = needed.max(min_height).min(max_height);

        Ok(self.viewport_height)
    }

    pub fn viewport_height(&self) -> u16 {
        self.viewport_height
    }

    pub fn anchor_col(&self) -> u16 {
        self.anchor_col
    }

    /// Queue finalized history lines for insertion into terminal scrollback.
    pub fn insert_history_lines(&mut self, lines: Vec<Line<'static>>) {
        self.pending_history_lines.extend(lines);
    }

    /// Draw a frame in the live viewport after flushing any queued history lines.
    pub fn draw<F>(&mut self, draw_fn: F) -> Result<()>
    where
        F: FnOnce(&mut custom_terminal::Frame),
    {
        let terminal = &mut self.terminal;
        let size = terminal.size().context("failed to query terminal size")?;
        if size.height == 0 || size.width == 0 {
            return Ok(());
        }

        let mut area = terminal.viewport_area;
        area.width = size.width;
        area.height = self.viewport_height.min(size.height).max(1);

        // If viewport grows past bottom, scroll everything above it upward.
        if area.bottom() > size.height {
            let scroll_by = area.bottom() - size.height;
            if scroll_by > 0 && area.top() > 0 {
                terminal
                    .backend_mut()
                    .scroll_region_up(0..area.top(), scroll_by)
                    .context("failed to scroll region up for viewport")?;
            }
            area.y = size.height.saturating_sub(area.height);
        }

        if area != terminal.viewport_area {
            terminal
                .clear()
                .context("failed clearing terminal before viewport resize")?;
            terminal.set_viewport_area(area);
        }

        if !self.pending_history_lines.is_empty() {
            let pending = std::mem::take(&mut self.pending_history_lines);
            super::insert_history::insert_history_lines(terminal, pending)
                .context("failed inserting history into scrollback")?;
        }

        terminal
            .draw(draw_fn)
            .context("failed drawing terminal frame")
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        if !self.keep_output {
            let _ = self.terminal.clear();
        }

        let _ = disable_raw_mode();
    }
}

fn initial_viewport_height(terminal_height: u16) -> u16 {
    let max_height = terminal_height
        .saturating_sub(VIEWPORT_BOTTOM_MARGIN)
        .max(1);
    MIN_VIEWPORT_HEIGHT.min(max_height)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_panic_hook_installation() {
        install_panic_hook();
        install_panic_hook();
    }

    #[test]
    fn initial_height_respects_terminal_space() {
        assert_eq!(initial_viewport_height(40), 10);
        assert_eq!(initial_viewport_height(9), 7);
        assert_eq!(initial_viewport_height(1), 1);
    }
}
