/// The vim-style interaction mode. `Normal` carries the pending numeric count
/// prefix — the `10` in `10j` — while it accumulates; it cannot exist in
/// `Search`, so a non-command mode is never in a half-typed count.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    /// Command mode: keys navigate, none type.
    Normal { count: Option<usize> },
    /// Text-entry mode: keystrokes edit the query (same as the non-modal UI).
    Search,
}

impl Mode {
    /// Uppercase label for the input's mode chip. Both labels are six columns
    /// wide, so the chip's width is constant.
    pub fn label(&self) -> &'static str {
        match self {
            Mode::Normal { .. } => "NORMAL",
            Mode::Search => "SEARCH",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[rstest]
    #[case::normal(Mode::Normal { count: None }, "NORMAL")]
    #[case::search(Mode::Search, "SEARCH")]
    fn label_names_the_mode_in_six_columns(#[case] mode: Mode, #[case] expected: &str) {
        assert_eq!(mode.label(), expected);
        assert_eq!(mode.label().chars().count(), 6, "labels must be six columns");
    }
}
