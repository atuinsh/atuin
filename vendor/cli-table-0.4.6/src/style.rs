use termcolor::{Color, ColorSpec};

/// Trait for modifying style of table and cells
pub trait Style {
    /// Used to set foreground color
    fn foreground_color(self, foreground_color: Option<Color>) -> Self;
    /// Used to set background color
    fn background_color(self, background_color: Option<Color>) -> Self;
    /// Used to set contents to be bold
    fn bold(self, bold: bool) -> Self;
    /// Used to set contents to be underlined
    fn underline(self, underline: bool) -> Self;
    /// Used to set contents to be italic
    fn italic(self, italic: bool) -> Self;
    /// Used to set high intensity version of a color specified
    fn intense(self, intense: bool) -> Self;
    /// Used to set contents to be dimmed
    fn dimmed(self, dimmed: bool) -> Self;
}

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct StyleStruct {
    pub(crate) foreground_color: Option<Color>,
    pub(crate) background_color: Option<Color>,
    pub(crate) bold: bool,
    pub(crate) underline: bool,
    pub(crate) italic: bool,
    pub(crate) intense: bool,
    pub(crate) dimmed: bool,
}

impl StyleStruct {
    pub(crate) fn color_spec(&self) -> ColorSpec {
        let mut color_spec = ColorSpec::new();

        color_spec.set_fg(self.foreground_color);
        color_spec.set_bg(self.background_color);
        color_spec.set_bold(self.bold);
        color_spec.set_underline(self.underline);
        color_spec.set_italic(self.italic);
        color_spec.set_intense(self.intense);
        color_spec.set_dimmed(self.dimmed);

        color_spec
    }
}

impl Style for StyleStruct {
    fn foreground_color(mut self, foreground_color: Option<Color>) -> Self {
        self.foreground_color = foreground_color;
        self
    }

    fn background_color(mut self, background_color: Option<Color>) -> Self {
        self.background_color = background_color;
        self
    }

    fn bold(mut self, bold: bool) -> Self {
        self.bold = bold;
        self
    }

    fn underline(mut self, underline: bool) -> Self {
        self.underline = underline;
        self
    }

    fn italic(mut self, italic: bool) -> Self {
        self.italic = italic;
        self
    }

    fn intense(mut self, intense: bool) -> Self {
        self.intense = intense;
        self
    }

    fn dimmed(mut self, dimmed: bool) -> Self {
        self.dimmed = dimmed;
        self
    }
}
