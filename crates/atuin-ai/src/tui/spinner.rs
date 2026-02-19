//! Spinner styles and configuration for TUI animations
//!
//! To experiment with different spinners, change `ACTIVE_SPINNER` below.

use std::time::Duration;

/// Active spinner style - change this to experiment with different styles
pub const ACTIVE_SPINNER: SpinnerStyle = SpinnerStyle::Dots;

/// Spinner style definitions
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpinnerStyle {
    /// Classic ASCII line spinner: / - \ |
    Line,
    /// Braille dots pattern
    Dots,
    /// Growing/shrinking dots
    Pulse,
    /// Simple arrow rotation
    Arrow,
    /// Block building
    Block,
}

impl SpinnerStyle {
    /// Get the frames for this spinner style
    pub const fn frames(&self) -> &'static [&'static str] {
        match self {
            SpinnerStyle::Line => &["/", "-", "\\", "|"],
            SpinnerStyle::Dots => &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"],
            SpinnerStyle::Pulse => &["·", "•", "●", "•"],
            SpinnerStyle::Arrow => &["←", "↖", "↑", "↗", "→", "↘", "↓", "↙"],
            SpinnerStyle::Block => &[
                "▏", "▎", "▍", "▌", "▋", "▊", "▉", "█", "▉", "▊", "▋", "▌", "▍", "▎", "▏",
            ],
        }
    }

    /// Get the recommended tick interval for this spinner style
    /// Faster spinners need shorter intervals to look smooth
    pub const fn tick_interval(&self) -> Duration {
        match self {
            SpinnerStyle::Line => Duration::from_millis(150),
            SpinnerStyle::Dots => Duration::from_millis(80),
            SpinnerStyle::Pulse => Duration::from_millis(200),
            SpinnerStyle::Arrow => Duration::from_millis(100),
            SpinnerStyle::Block => Duration::from_millis(80),
        }
    }

    /// Get the frame at the given index (wraps around)
    pub fn frame_at(&self, index: usize) -> &'static str {
        let frames = self.frames();
        frames[index % frames.len()]
    }

    /// Get the number of frames in this spinner
    pub fn frame_count(&self) -> usize {
        self.frames().len()
    }
}

/// Get the active spinner's frame at the given index
pub fn active_frame(index: usize) -> &'static str {
    ACTIVE_SPINNER.frame_at(index)
}

/// Get the active spinner's tick interval
pub fn active_tick_interval() -> Duration {
    ACTIVE_SPINNER.tick_interval()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_frame_wrapping() {
        let style = SpinnerStyle::Line;
        assert_eq!(style.frame_at(0), "/");
        assert_eq!(style.frame_at(4), "/"); // wraps
        assert_eq!(style.frame_at(5), "-");
    }

    #[test]
    fn test_all_styles_have_frames() {
        let styles = [
            SpinnerStyle::Line,
            SpinnerStyle::Dots,
            SpinnerStyle::Pulse,
            SpinnerStyle::Arrow,
            SpinnerStyle::Block,
        ];
        for style in styles {
            assert!(!style.frames().is_empty());
            assert!(style.tick_interval().as_millis() > 0);
        }
    }
}
