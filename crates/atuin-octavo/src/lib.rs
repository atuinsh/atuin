#![deny(unsafe_code)]

pub mod hub;
pub mod pb;
pub mod queue;
pub mod shares;

/// Whether commands, and their output, go up to Atuin for Teams, whatever a config file's
/// `upload.history` says.
///
/// TODO(markovejnovic): history uploads are off until Atuin for Teams takes them again. The code
/// that sends them stays, behind this.
pub const UPLOADS_HISTORY: bool = false;
