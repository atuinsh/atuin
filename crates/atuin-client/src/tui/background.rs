//! Whether the terminal's background is light or dark, so that a view can pick colours that read
//! on it.

use atuin_common::env::var_nonempty;
use terminal_colorsaurus::{QueryOptions, ThemeMode, theme_mode};

/// How light the terminal's background is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Background {
    Light,
    Dark,
}

impl Background {
    /// The terminal's background, or `None` if neither the terminal nor `COLORFGBG` says.
    ///
    /// Call this before a view starts reading input, because the terminal's answer arrives as
    /// input. A terminal that doesn't answer can hold this up for as long as a second.
    #[must_use]
    pub fn detect() -> Option<Self> {
        // The terminal's answer comes first because few terminals set `COLORFGBG`, and a
        // `COLORFGBG` that a tmux session or an SSH login carried over can describe another
        // terminal.
        Self::queried().or_else(|| {
            let colorfgbg = var_nonempty("COLORFGBG")?;
            Self::from_colorfgbg(colorfgbg.to_str()?)
        })
    }

    /// The background that the terminal reports.
    fn queried() -> Option<Self> {
        match theme_mode(QueryOptions::default()) {
            Ok(ThemeMode::Light) => Some(Self::Light),
            Ok(ThemeMode::Dark) => Some(Self::Dark),
            Err(_) => None,
        }
    }

    /// The background that a `COLORFGBG` value gives.
    ///
    /// The value is "fg;bg" or "fg;default;bg" in the 16 ANSI colours, of which 7 and 15 are
    /// white.
    fn from_colorfgbg(value: &str) -> Option<Self> {
        match value.rsplit(';').next()?.parse::<u8>().ok()? {
            7 | 15 => Some(Self::Light),
            0..=15 => Some(Self::Dark),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::Background;

    #[rstest]
    #[case::dark("15;0", Some(Background::Dark))]
    #[case::light("0;15", Some(Background::Light))]
    #[case::light_grey("0;7", Some(Background::Light))]
    #[case::light_with_default("0;default;7", Some(Background::Light))]
    #[case::dark_with_default("7;default;0", Some(Background::Dark))]
    #[case::past_the_ansi_colours("0;16", None)]
    #[case::unreadable("light", None)]
    #[case::empty_background("0;", None)]
    fn colorfgbg_gives_the_background(#[case] value: &str, #[case] expected: Option<Background>) {
        assert_eq!(Background::from_colorfgbg(value), expected);
    }
}
