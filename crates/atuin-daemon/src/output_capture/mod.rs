mod engine;
mod persistence;

use atuin_client::history::HistoryId;
use atuin_common::string::highlighted::HighlightedString;
pub use engine::OutputCaptureEngine;
pub use persistence::{CaptureError, DeleteOutputError, GetOutputError, OutputStore};

#[derive(Debug)]
pub struct OutputMatch {
    pub history_id: HistoryId,
    pub output: HighlightedString,
    pub score: f64,
}
