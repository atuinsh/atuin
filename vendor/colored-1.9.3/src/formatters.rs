
use color::Color;
use style::Style;

pub trait ColoringFormatter {
    fn format(out: String, fg: Color, bg: Color, style: Style) -> String;
}

pub struct NoColor;

impl ColoringFormatter for NoColor {
    fn format(out: String, _fg: Color, _bg: Color, _style: Style) -> String {
        out
    }
}
