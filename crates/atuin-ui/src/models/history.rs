use std::collections::VecDeque;
use std::future::Future;
use std::ops::Range;

/// Rows kept loaded beyond the visible window, above and below, so scrolling a
/// little doesn't immediately re-fetch.
const MARGIN: usize = 32;

/// A history entry, in the shape the UI renders. The host maps its own
/// `History` into this at the loading boundary, so this crate stays decoupled.
#[derive(Clone, Debug)]
pub struct HistoryRow {
    /// Opaque row id (the uuid), for future actions (accept/delete).
    pub id: String,
    pub command: String,
    /// Unix seconds at which the command ran (for the relative "… ago" time).
    pub timestamp: i64,
    /// Nanoseconds; `-1` means still running.
    pub duration: i64,
    pub exit: i64,
}

/// A virtualized view over a possibly-huge history list. Holds only a bounded,
/// contiguous *window* of materialized rows plus the logical `total` and the
/// selection/scroll position — never the whole list. The app fetches windows on
/// demand (see [`HistorySource`]) as the selection moves.
#[derive(Default)]
pub struct HistoryList {
    total: usize,
    /// Materialized rows for logical indices `[window_start, window_start + len)`.
    window: VecDeque<HistoryRow>,
    window_start: usize,
    selected: usize,
    /// Logical index of the first visible row.
    offset: usize,
    /// Rows the viewport can show; recorded from the render area each frame.
    viewport_height: u16,
}

impl HistoryList {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn total(&self) -> usize {
        self.total
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    pub fn offset(&self) -> usize {
        self.offset
    }

    /// The row at logical `index`, if it is inside the loaded window.
    pub fn row(&self, index: usize) -> Option<&HistoryRow> {
        index
            .checked_sub(self.window_start)
            .and_then(|i| self.window.get(i))
    }

    pub fn set_viewport_height(&mut self, height: u16) {
        self.viewport_height = height;
        self.clamp_offset();
    }

    pub fn set_total(&mut self, total: usize) {
        self.total = total;
        if self.selected >= total {
            self.selected = total.saturating_sub(1);
        }
        self.clamp_offset();
    }

    // --- navigation (logical, pure) -------------------------------------------

    pub fn select_next(&mut self) {
        self.select_to(self.selected.saturating_add(1));
    }

    pub fn select_prev(&mut self) {
        self.select_to(self.selected.saturating_sub(1));
    }

    pub fn page_down(&mut self) {
        self.select_to(self.selected.saturating_add(self.page()));
    }

    pub fn page_up(&mut self) {
        self.select_to(self.selected.saturating_sub(self.page()));
    }

    pub fn select_first(&mut self) {
        self.select_to(0);
    }

    pub fn select_last(&mut self) {
        self.select_to(self.total.saturating_sub(1));
    }

    fn page(&self) -> usize {
        (self.viewport_height as usize).max(1)
    }

    fn select_to(&mut self, index: usize) {
        if self.total == 0 {
            return;
        }
        self.selected = index.min(self.total - 1);
        self.clamp_offset();
    }

    /// Keep the selection inside the viewport, and the viewport inside the list.
    fn clamp_offset(&mut self) {
        let h = self.viewport_height as usize;
        if h == 0 {
            self.offset = self.selected;
            return;
        }
        if self.selected < self.offset {
            self.offset = self.selected;
        } else if self.selected >= self.offset + h {
            self.offset = self.selected + 1 - h;
        }
        self.offset = self.offset.min(self.total.saturating_sub(h));
    }

    // --- windowing (the loading seam) -----------------------------------------

    /// The logical range that should be resident: the visible range plus a
    /// prefetch margin, clamped to the list.
    pub fn desired_range(&self) -> Range<usize> {
        let h = self.viewport_height as usize;
        let start = self.offset.saturating_sub(MARGIN);
        let end = (self.offset + h + MARGIN).min(self.total);
        start..end
    }

    /// Whether the current window already covers `range`.
    pub fn has(&self, range: Range<usize>) -> bool {
        range.start >= self.window_start && range.end <= self.window_start + self.window.len()
    }

    /// Replace the resident window with a freshly-loaded run for logical
    /// `[start, start + rows.len())`. Bounded because callers only ever load
    /// [`desired_range`](Self::desired_range).
    pub fn apply(&mut self, start: usize, rows: Vec<HistoryRow>) {
        self.window_start = start;
        self.window = VecDeque::from(rows);
    }
}

/// Supplies history rows on demand — implemented by the host over its database,
/// so this crate never touches `atuin-client`. Loads run as async tasks, so the
/// source is cloned into each task (hence `Clone + Send + 'static`).
pub trait HistorySource: Clone + Send + 'static {
    /// Total number of rows in the underlying list.
    fn total(&self) -> impl Future<Output = usize> + Send;

    /// The rows for the logical `range`.
    fn load(&self, range: Range<usize>) -> impl Future<Output = Vec<HistoryRow>> + Send;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(i: usize) -> HistoryRow {
        HistoryRow {
            id: i.to_string(),
            command: format!("cmd {i}"),
            timestamp: 0,
            duration: 0,
            exit: 0,
        }
    }

    /// Mimic the app's load step: whenever the desired range isn't resident,
    /// fetch and apply it.
    fn ensure_loaded(list: &mut HistoryList) {
        let range = list.desired_range();
        if !range.is_empty() && !list.has(range.clone()) {
            let rows = range.clone().map(row).collect();
            list.apply(range.start, rows);
        }
    }

    #[test]
    fn window_stays_bounded_scrolling_millions() {
        let mut list = HistoryList::new();
        list.set_viewport_height(20);
        list.set_total(5_000_000);
        ensure_loaded(&mut list);

        for _ in 0..2000 {
            list.select_next();
            ensure_loaded(&mut list);
        }

        assert!(
            list.window.len() <= 20 + 2 * MARGIN,
            "window not bounded: {}",
            list.window.len()
        );
        assert!(list.selected() > 0);
        assert!(
            list.row(list.selected()).is_some(),
            "selected row must be resident"
        );
    }

    #[test]
    fn selection_stays_visible() {
        let mut list = HistoryList::new();
        list.set_viewport_height(10);
        list.set_total(100);

        list.select_last();
        assert_eq!(list.selected(), 99);
        assert!(list.selected() >= list.offset() && list.selected() < list.offset() + 10);

        list.select_first();
        assert_eq!(list.selected(), 0);
        assert_eq!(list.offset(), 0);
    }

    #[test]
    fn empty_list_is_safe() {
        let mut list = HistoryList::new();
        list.set_viewport_height(10);
        list.set_total(0);
        list.select_next();
        list.page_down();
        assert_eq!(list.selected(), 0);
        assert!(list.row(0).is_none());
        assert!(list.desired_range().is_empty());
    }
}
