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

    #[test]
    fn labels_are_six_columns() {
        assert_eq!(Mode::Normal { count: None }.label(), "NORMAL");
        assert_eq!(Mode::Search.label(), "SEARCH");
        assert_eq!(Mode::Normal { count: None }.label().chars().count(), 6);
        assert_eq!(Mode::Search.label().chars().count(), 6);
    }
}
