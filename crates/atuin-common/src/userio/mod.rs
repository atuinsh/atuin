//! Utilities for talking to a user.

pub mod log;
mod out;
mod prompt;

pub use out::{Output, out, outln};
pub use prompt::{Confirm, DefaultAnswer, Prompt, Secret};
