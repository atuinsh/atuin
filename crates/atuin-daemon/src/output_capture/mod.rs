mod engine;
mod persistence;

pub use engine::OutputCaptureEngine;
pub use persistence::{
    CaptureError, DeleteOutputError, GetOutputError, OutputLine, OutputMatch, OutputStore,
};
