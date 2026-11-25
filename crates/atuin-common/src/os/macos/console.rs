//! Who is logged in at the Mac's screen.

use std::os::unix::fs::MetadataExt as _;

/// Whether the user of this process is logged in at the Mac's screen.
///
/// Only that user has a graphical session, in which launchd runs the user's agents and in
/// which System Settings can open.
#[must_use]
pub fn is_at_screen() -> bool {
    // The user at the screen owns the console.
    std::fs::metadata("/dev/console").is_ok_and(|console| console.uid() == crate::os::unix::uid())
}
