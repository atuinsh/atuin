mod backend;
mod engine;

use atuin_client::history::HistoryId;
use atuin_common::string::highlighted::HighlightedString;
pub use backend::{CaptureError, DeleteOutputError, GetOutputError, OutputStore};
pub use engine::OutputCaptureEngine;

#[derive(Debug)]
pub struct OutputMatch {
    pub history_id: HistoryId,
    pub output: HighlightedString,
    pub score: f64,
}
