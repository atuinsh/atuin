/// A style is a collection of properties that can format a string
/// using ANSI escape codes.
#[derive(PartialEq, Clone, Copy)]
pub struct Style {

    /// The style's foreground colour, if it has one.
    pub foreground: Option<Colour>,

    /// The style's background colour, if it has one.
    pub background: Option<Colour>,

    /// Whether this style is bold.
    pub is_bold: bool,

    /// Whether this style is dimmed.
    pub is_dimmed: bool,

    /// Whether this style is italic.
    pub is_italic: bool,

    /// Whether this style is underlined.
    pub is_underline: bool,

    /// Whether this style is blinking.
    pub is_blink: bool,

    /// Whether this style has reverse colours.
    pub is_reverse: bool,

    /// Whether this style is hidden.
    pub is_hidden: bool,

    /// Whether this style is struckthrough.
    pub is_strikethrough: bool
}

impl Style {
    /// Creates a new Style with no differences.
    pub fn new() -> Style {
        Style::default()
    }

    /// Returns a `Style` with the bold property set.
    pub fn bold(&self) -> Style {
        Style { is_bold: true, .. *self }
    }

    /// Returns a `Style` with the dimmed property set.
    pub fn dimmed(&self) -> Style {
        Style { is_dimmed: true, .. *self }
    }

    /// Returns a `Style` with the italic property set.
    pub fn italic(&self) -> Style {
        Style { is_italic: true, .. *self }
    }

    /// Returns a `Style` with the underline property set.
    pub fn underline(&self) -> Style {
        Style { is_underline: true, .. *self }
    }

    /// Returns a `Style` with the blink property set.
    pub fn blink(&self) -> Style {
        Style { is_blink: true, .. *self }
    }

    /// Returns a `Style` with the reverse property set.
    pub fn reverse(&self) -> Style {
        Style { is_reverse: true, .. *self }
    }

    /// Returns a `Style` with the hidden property set.
    pub fn hidden(&self) -> Style {
        Style { is_hidden: true, .. *self }
    }

    /// Returns a `Style` with the hidden property set.
    pub fn strikethrough(&self) -> Style {
        Style { is_strikethrough: true, .. *self }
    }

    /// Returns a `Style` with the foreground colour property set.
    pub fn fg(&self, foreground: Colour) -> Style {
        Style { foreground: Some(foreground), .. *self }
    }

    /// Returns a `Style` with the background colour property set.
    pub fn on(&self, background: Colour) -> Style {
        Style { background: Some(background), .. *self }
    }

    /// Return true if this `Style` has no actual styles, and can be written
    /// without any control characters.
    pub fn is_plain(self) -> bool {
        self == Style::default()
    }
}

impl Default for Style {

    /// Returns a style with *no* properties set. Formatting text using this
    /// style returns the exact same text.
    ///
    /// ```
    /// use ansi_term::Style;
    /// assert_eq!(None,  Style::default().foreground);
    /// assert_eq!(None,  Style::default().background);
    /// assert_eq!(false, Style::default().is_bold);
    /// assert_eq!("txt", Style::default().paint("txt").to_string());
    /// ```
    fn default() -> Style {
        Style {
            foreground: None,
            background: None,
            is_bold: false,
            is_dimmed: false,
            is_italic: false,
            is_underline: false,
            is_blink: false,
            is_reverse: false,
            is_hidden: false,
            is_strikethrough: false,
        }
    }
}


// ---- colours ----

/// A colour is one specific type of ANSI escape code, and can refer
/// to either the foreground or background colour.
///
/// These use the standard numeric sequences.
/// See <http://invisible-island.net/xterm/ctlseqs/ctlseqs.html>
#[derive(PartialEq, Clone, Copy, Debug)]
pub enum Colour {

    /// Colour #0 (foreground code `30`, background code `40`).
    ///
    /// This is not necessarily the background colour, and using it as one may
    /// render the text hard to read on terminals with dark backgrounds.
    Black,

    /// Colour #1 (foreground code `31`, background code `41`).
    Red,

    /// Colour #2 (foreground code `32`, background code `42`).
    Green,

    /// Colour #3 (foreground code `33`, background code `43`).
    Yellow,

    /// Colour #4 (foreground code `34`, background code `44`).
    Blue,

    /// Colour #5 (foreground code `35`, background code `45`).
    Purple,

    /// Colour #6 (foreground code `36`, background code `46`).
    Cyan,

    /// Colour #7 (foreground code `37`, background code `47`).
    ///
    /// As above, this is not necessarily the foreground colour, and may be
    /// hard to read on terminals with light backgrounds.
    White,

    /// A colour number from 0 to 255, for use in 256-colour terminal
    /// environments.
    ///
    /// - Colours 0 to 7 are the `Black` to `White` variants respectively.
    ///   These colours can usually be changed in the terminal emulator.
    /// - Colours 8 to 15 are brighter versions of the eight colours above.
    ///   These can also usually be changed in the terminal emulator, or it
    ///   could be configured to use the original colours and show the text in
    ///   bold instead. It varies depending on the program.
    /// - Colours 16 to 231 contain several palettes of bright colours,
    ///   arranged in six squares measuring six by six each.
    /// - Colours 232 to 255 are shades of grey from black to white.
    ///
    /// It might make more sense to look at a [colour chart][cc].
    ///
    /// [cc]: https://upload.wikimedia.org/wikipedia/en/1/15/Xterm_256color_chart.svg
    Fixed(u8),

    /// A 24-bit RGB color, as specified by ISO-8613-3.
    RGB(u8, u8, u8),
}


impl Colour {
    /// Return a `Style` with the foreground colour set to this colour.
    pub fn normal(self) -> Style {
        Style { foreground: Some(self), .. Style::default() }
    }

    /// Returns a `Style` with the bold property set.
    pub fn bold(self) -> Style {
        Style { foreground: Some(self), is_bold: true, .. Style::default() }
    }

    /// Returns a `Style` with the dimmed property set.
    pub fn dimmed(self) -> Style {
        Style { foreground: Some(self), is_dimmed: true, .. Style::default() }
    }

    /// Returns a `Style` with the italic property set.
    pub fn italic(self) -> Style {
        Style { foreground: Some(self), is_italic: true, .. Style::default() }
    }

    /// Returns a `Style` with the underline property set.
    pub fn underline(self) -> Style {
        Style { foreground: Some(self), is_underline: true, .. Style::default() }
    }

    /// Returns a `Style` with the blink property set.
    pub fn blink(self) -> Style {
        Style { foreground: Some(self), is_blink: true, .. Style::default() }
    }

    /// Returns a `Style` with the reverse property set.
    pub fn reverse(self) -> Style {
        Style { foreground: Some(self), is_reverse: true, .. Style::default() }
    }

    /// Returns a `Style` with the hidden property set.
    pub fn hidden(self) -> Style {
        Style { foreground: Some(self), is_hidden: true, .. Style::default() }
    }

    /// Returns a `Style` with the strikethrough property set.
    pub fn strikethrough(self) -> Style {
        Style { foreground: Some(self), is_strikethrough: true, .. Style::default() }
    }

    /// Returns a `Style` with the background colour property set.
    pub fn on(self, background: Colour) -> Style {
        Style { foreground: Some(self), background: Some(background), .. Style::default() }
    }
}

impl From<Colour> for Style {

    /// You can turn a `Colour` into a `Style` with the foreground colour set
    /// with the `From` trait.
    ///
    /// ```
    /// use ansi_term::{Style, Colour};
    /// let green_foreground = Style::default().fg(Colour::Green);
    /// assert_eq!(green_foreground, Colour::Green.normal());
    /// assert_eq!(green_foreground, Colour::Green.into());
    /// assert_eq!(green_foreground, Style::from(Colour::Green));
    /// ```
    fn from(colour: Colour) -> Style {
        colour.normal()
    }
}
