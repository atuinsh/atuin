//! Clearing the screen.

use std::fmt;

derive_csi_sequence!("Clear the entire screen.", All, "2J");
derive_csi_sequence!("Clear everything after the cursor.", AfterCursor, "J");
derive_csi_sequence!("Clear everything before the cursor.", BeforeCursor, "1J");
derive_csi_sequence!("Clear the current line.", CurrentLine, "2K");
derive_csi_sequence!("Clear from cursor to newline.", UntilNewline, "K");
