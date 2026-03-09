use ratatui::layout::Rect;

/// Maximum popup height (lines). Keeps context visible around the popup.
const MAX_POPUP_HEIGHT: u16 = 24;

/// Minimum usable popup height.
const MIN_POPUP_HEIGHT: u16 = 5;

/// Initial popup height — just enough for input + a small response.
const INITIAL_POPUP_HEIGHT: u16 = 5;

/// Margin around the card in popup mode.
pub(crate) const POPUP_MARGIN: u16 = 0;

/// Screen state captured from atuin-hex's screen server.
pub struct SavedScreen {
    #[allow(dead_code)]
    pub rows: u16,
    #[allow(dead_code)]
    pub cols: u16,
    pub cursor_row: u16,
    pub cursor_col: u16,
    /// Pre-formatted ANSI bytes for each screen row, ready to write to stdout.
    pub rows_data: Vec<Vec<u8>>,
}

/// Popup mode state: saved screen + computed placement.
pub struct PopupState {
    pub saved_screen: SavedScreen,
    /// Maximum rect computed from placement (the ceiling for growth).
    pub max_rect: Rect,
    /// Current rect — starts small, grows as content arrives.
    pub current_rect: Rect,
    pub scroll_offset: u16,
    /// True when the popup renders above the cursor (input at bottom of card).
    pub render_above: bool,
}

impl PopupState {
    /// Resize the popup to fit `needed` lines of content.
    ///
    /// Grows or shrinks the popup as needed (clamped to max_rect / INITIAL_POPUP_HEIGHT).
    /// When growing, clears the new rect area. When shrinking, restores freed rows
    /// from the saved screen data.
    ///
    /// Returns `Some(new_rect)` if the size changed (caller must resize terminal),
    /// or `None` if no change is needed.
    pub fn fit_to(&mut self, needed: u16) -> Option<Rect> {
        let new_height = needed.clamp(INITIAL_POPUP_HEIGHT, self.max_rect.height);
        if new_height == self.current_rect.height {
            return None;
        }

        let old_rect = self.current_rect;
        let growing = new_height > old_rect.height;

        if self.render_above {
            let new_y = self.max_rect.y + self.max_rect.height - new_height;
            self.current_rect = Rect::new(old_rect.x, new_y, old_rect.width, new_height);
        } else {
            self.current_rect = Rect::new(old_rect.x, old_rect.y, old_rect.width, new_height);
        }

        if growing {
            // Clear the entire new rect so the new Terminal doesn't leave
            // ghost content from the old card.
            self.clear_rows(
                self.current_rect.y,
                self.current_rect.y + self.current_rect.height,
            );
        } else {
            // Shrinking: restore freed rows from saved screen data, then
            // clear the new (smaller) rect for the re-rendered card.
            self.restore_rows(&old_rect);
            self.clear_rows(
                self.current_rect.y,
                self.current_rect.y + self.current_rect.height,
            );
        }

        Some(self.current_rect)
    }

    /// Clear a range of terminal rows within the popup width.
    fn clear_rows(&self, from_row: u16, to_row: u16) {
        use crossterm::cursor::MoveTo;
        use crossterm::execute;
        use crossterm::style::{Attribute, SetAttribute};
        use std::io::{Write, stdout};

        let mut out = stdout();
        for row in from_row..to_row {
            let _ = execute!(
                out,
                MoveTo(self.current_rect.x, row),
                SetAttribute(Attribute::Reset)
            );
            let _ = write!(
                out,
                "{:width$}",
                "",
                width = self.current_rect.width as usize
            );
        }
        let _ = out.flush();
    }

    /// Restore rows that were freed by shrinking — the rows in old_rect
    /// that are no longer covered by current_rect.
    fn restore_rows(&self, old_rect: &Rect) {
        use crossterm::cursor::MoveTo;
        use crossterm::execute;
        use crossterm::style::{Attribute, SetAttribute};
        use std::io::{Write, stdout};

        let mut out = stdout();

        // Determine which rows are freed
        let (freed_start, freed_end) = if self.render_above {
            // Shrinking from above: freed rows are at the old top
            (old_rect.y, self.current_rect.y)
        } else {
            // Shrinking from below: freed rows are at the old bottom
            (
                self.current_rect.y + self.current_rect.height,
                old_rect.y + old_rect.height,
            )
        };

        for row in freed_start..freed_end {
            let source_row = (row + self.scroll_offset) as usize;

            // Clear the popup region
            let _ = execute!(out, MoveTo(old_rect.x, row), SetAttribute(Attribute::Reset),);
            let _ = write!(out, "{:width$}", "", width = old_rect.width as usize);

            // Write back saved row data from column 0
            let _ = execute!(out, MoveTo(0, row));
            if let Some(row_bytes) = self.saved_screen.rows_data.get(source_row) {
                let _ = out.write_all(row_bytes);
            }
        }
        let _ = out.flush();
    }
}

/// Try to set up popup overlay mode.
///
/// Checks for `ATUIN_HEX_SOCKET`, fetches screen state, computes placement,
/// and scrolls the terminal if needed. Returns `None` if popup mode is not
/// available (no socket, fetch failed, etc.), in which case the caller should
/// fall back to inline mode.
pub fn try_setup_popup() -> Option<PopupState> {
    use std::io::Write;

    let socket_path = std::env::var("ATUIN_HEX_SOCKET").ok()?;
    let saved = fetch_screen_state(&socket_path)?;

    let (term_cols, term_rows) = crossterm::terminal::size().unwrap_or((saved.cols, saved.rows));
    // Full-width popup with margin for visual separation
    let popup_width = term_cols;
    let (rect, scroll, render_above) = compute_popup_placement(
        saved.cursor_row,
        saved.cursor_col,
        term_rows,
        term_cols,
        popup_width,
    );

    // Scroll terminal up if needed to make room for the popup
    if scroll > 0 {
        let mut stdout = std::io::stdout();
        let _ = crossterm::execute!(stdout, crossterm::cursor::MoveTo(0, term_rows - 1));
        for _ in 0..scroll {
            let _ = writeln!(stdout);
        }
        let _ = stdout.flush();
    }

    // Start with a small rect that grows as content arrives
    let initial_height = INITIAL_POPUP_HEIGHT.min(rect.height);
    let current_rect = if render_above {
        // Anchor at the bottom of max_rect (near cursor), grow upward
        Rect::new(
            rect.x,
            rect.y + rect.height - initial_height,
            rect.width,
            initial_height,
        )
    } else {
        // Anchor at the top of max_rect (near cursor), grow downward
        Rect::new(rect.x, rect.y, rect.width, initial_height)
    };

    Some(PopupState {
        saved_screen: saved,
        max_rect: rect,
        current_rect,
        scroll_offset: scroll,
        render_above,
    })
}

/// Restore the screen area that was covered by the popup.
///
/// Clears the popup region, then writes pre-formatted per-row ANSI bytes from
/// column 0 to correctly restore wide characters, colors, and all attributes.
pub fn restore(state: &PopupState) {
    use crossterm::cursor::MoveTo;
    use crossterm::execute;
    use crossterm::style::{Attribute, SetAttribute};
    use std::io::{Write, stdout};

    let saved = &state.saved_screen;
    let popup_rect = state.current_rect;
    let scroll_offset = state.scroll_offset;

    let mut stdout = stdout();

    for dy in 0..popup_rect.height {
        let target_row = popup_rect.y + dy;
        let source_row = (target_row + scroll_offset) as usize;

        // Clear only the popup region with spaces
        let _ = execute!(
            stdout,
            MoveTo(popup_rect.x, target_row),
            SetAttribute(Attribute::Reset),
        );
        let _ = write!(stdout, "{:width$}", "", width = popup_rect.width as usize);

        // Write back full row ANSI data from column 0
        let _ = execute!(stdout, MoveTo(0, target_row));
        if let Some(row_bytes) = saved.rows_data.get(source_row) {
            let _ = stdout.write_all(row_bytes);
        }
    }

    // Restore cursor position (adjusted for any scrolling)
    let _ = execute!(
        stdout,
        MoveTo(
            saved.cursor_col,
            saved.cursor_row.saturating_sub(scroll_offset)
        )
    );
    let _ = stdout.flush();
}

/// Connect to atuin-hex's Unix socket and fetch the current screen state.
///
/// The wire format is:
/// ```text
/// [rows: u16 BE][cols: u16 BE][cursor_row: u16 BE][cursor_col: u16 BE]
/// [row_0_len: u32 BE][row_0_bytes...]
/// [row_1_len: u32 BE][row_1_bytes...]
/// ...
/// ```
fn fetch_screen_state(socket_path: &str) -> Option<SavedScreen> {
    use std::io::Read;
    use std::os::unix::net::UnixStream;
    use std::time::Duration;

    let mut stream = UnixStream::connect(socket_path).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(2))).ok()?;

    let mut data = Vec::new();
    stream.read_to_end(&mut data).ok()?;

    if data.len() < 8 {
        return None;
    }

    let rows = u16::from_be_bytes([data[0], data[1]]);
    let cols = u16::from_be_bytes([data[2], data[3]]);
    let cursor_row = u16::from_be_bytes([data[4], data[5]]);
    let cursor_col = u16::from_be_bytes([data[6], data[7]]);

    let mut rows_data = Vec::with_capacity(rows as usize);
    let mut offset = 8;
    while offset + 4 <= data.len() {
        let row_len = u32::from_be_bytes([
            data[offset],
            data[offset + 1],
            data[offset + 2],
            data[offset + 3],
        ]) as usize;
        offset += 4;
        if offset + row_len > data.len() {
            break;
        }
        rows_data.push(data[offset..offset + row_len].to_vec());
        offset += row_len;
    }

    Some(SavedScreen {
        rows,
        cols,
        cursor_row,
        cursor_col,
        rows_data,
    })
}

/// Compute popup placement for the AI card.
///
/// Positions the popup near the cursor: below if there's room, above otherwise.
/// Uses a capped height (MAX_POPUP_HEIGHT) so the popup doesn't fill the screen.
///
/// Returns `(popup_rect, scroll_offset, render_above)`:
/// - `render_above`: true when popup is above cursor (input should be at bottom)
/// - `scroll_offset`: lines the caller should scroll the terminal up
fn compute_popup_placement(
    cursor_row: u16,
    cursor_col: u16,
    term_rows: u16,
    term_cols: u16,
    card_width: u16,
) -> (Rect, u16, bool) {
    // Horizontal: anchor card near cursor, clamp to screen
    let popup_w = card_width.min(term_cols);
    let preferred_x = cursor_col.saturating_sub(2);
    let max_x = term_cols.saturating_sub(popup_w);
    let popup_x = preferred_x.min(max_x);

    // Vertical: use a reasonable height, not the full terminal
    let max_h = MAX_POPUP_HEIGHT
        .min(term_rows.saturating_sub(2))
        .max(MIN_POPUP_HEIGHT);
    let space_above = cursor_row;
    let space_below = term_rows.saturating_sub(cursor_row);

    if max_h <= space_below {
        // Fits below cursor — input at top (close to prompt)
        let popup_y = cursor_row;
        (Rect::new(popup_x, popup_y, popup_w, max_h), 0, false)
    } else if max_h <= space_above {
        // Fits above cursor — input at bottom (close to prompt)
        let popup_y = cursor_row.saturating_sub(max_h);
        (Rect::new(popup_x, popup_y, popup_w, max_h), 0, true)
    } else {
        // Neither side fits fully — use whichever side has more space,
        // scrolling the terminal if needed to reach MIN_POPUP_HEIGHT.
        let render_above = space_above > space_below;
        let available = if render_above {
            space_above
        } else {
            space_below
        };
        let h = available.max(MIN_POPUP_HEIGHT).min(max_h);
        let scroll = h.saturating_sub(available);
        let popup_y = if render_above {
            cursor_row.saturating_sub(h + scroll)
        } else {
            cursor_row.saturating_sub(scroll)
        };
        (
            Rect::new(popup_x, popup_y, popup_w, h),
            scroll,
            render_above,
        )
    }
}
