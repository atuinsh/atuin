mod engine;
mod persistence;

use atuin_client::history::HistoryId;
use atuin_common::string::highlighted::HighlightedString;
pub use engine::OutputCaptureEngine;
pub use persistence::{CaptureError, DeleteOutputError, GetOutputError, OutputStore};

/// One search hit: the lines around each match in one command's output.
#[derive(Debug)]
pub struct OutputMatch {
    pub history_id: HistoryId,
    /// Ascending; a gap in `line` between neighbours is output that was not shown.
    pub lines: Vec<OutputLine>,
    /// Relevance -- higher is more relevant.
    pub score: f64,
}

#[derive(Debug)]
pub struct OutputLine {
    /// The line's position in the output: 0-based from the start, or negative -- counting back
    /// from the end -- for a line of the kept tail of an output whose middle was discarded.
    pub line: i64,
    pub content: HighlightedString,
}
