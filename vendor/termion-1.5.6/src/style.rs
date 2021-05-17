//! Text styling management.

use std::fmt;

derive_csi_sequence!("Reset SGR parameters.", Reset, "m");
derive_csi_sequence!("Bold text.", Bold, "1m");
derive_csi_sequence!("Fainted text (not widely supported).", Faint, "2m");
derive_csi_sequence!("Italic text.", Italic, "3m");
derive_csi_sequence!("Underlined text.", Underline, "4m");
derive_csi_sequence!("Blinking text (not widely supported).", Blink, "5m");
derive_csi_sequence!("Inverted colors (negative mode).", Invert, "7m");
derive_csi_sequence!("Crossed out text (not widely supported).", CrossedOut, "9m");
derive_csi_sequence!("Undo bold text.", NoBold, "21m");
derive_csi_sequence!("Undo fainted text (not widely supported).", NoFaint, "22m");
derive_csi_sequence!("Undo italic text.", NoItalic, "23m");
derive_csi_sequence!("Undo underlined text.", NoUnderline, "24m");
derive_csi_sequence!("Undo blinking text (not widely supported).", NoBlink, "25m");
derive_csi_sequence!("Undo inverted colors (negative mode).", NoInvert, "27m");
derive_csi_sequence!("Undo crossed out text (not widely supported).",
                     NoCrossedOut,
                     "29m");
derive_csi_sequence!("Framed text (not widely supported).", Framed, "51m");
