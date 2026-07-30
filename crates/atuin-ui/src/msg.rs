use ratatui::crossterm::event::KeyEvent;

use crate::models::HistoryRow;

/// A message entering the runtime — the only way application state changes.
///
/// The runtime translates the outside world (terminal input) into these, and
/// async commands fold their results back in as these; `App::update` applies
/// them to the model.
pub enum Msg {
    /// A key was pressed.
    Key(KeyEvent),
    /// The terminal was resized to (columns, rows).
    Resize(u16, u16),
    /// The total number of history rows became known.
    HistoryTotal(usize),
    /// A window of history rows loaded for logical `[start, start + rows.len())`.
    HistoryLoaded { start: usize, rows: Vec<HistoryRow> },
    /// Results for a completed search of `query`.
    SearchResults { query: String, rows: Vec<HistoryRow> },
}
