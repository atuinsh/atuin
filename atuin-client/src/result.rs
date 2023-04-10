use crate::history::History;

// Return a single instance of history, but include some context.
// EG how many other history items we have that are the same
// Can be extended to include match context, when we use actual
// seach engines
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryResult {
    pub history: History,
    pub count: u64,
}
