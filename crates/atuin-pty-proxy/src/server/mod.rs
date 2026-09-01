pub mod cli;

#[cfg(unix)]
mod unix;

#[cfg(unix)]
pub use unix::{
    capture::{CommandCapture, CommandCaptureSink},
    pty_proxy::init_script,
};
