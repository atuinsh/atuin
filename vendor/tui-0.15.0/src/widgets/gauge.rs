use crate::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
    symbols,
    text::{Span, Spans},
    widgets::{Block, Widget},
};

/// A widget to display a task progress.
///
/// # Examples:
///
/// ```
/// # use tui::widgets::{Widget, Gauge, Block, Borders};
/// # use tui::style::{Style, Color, Modifier};
/// Gauge::default()
///     .block(Block::default().borders(Borders::ALL).title("Progress"))
///     .gauge_style(Style::default().fg(Color::White).bg(Color::Black).add_modifier(Modifier::ITALIC))
///     .percent(20);
/// ```
#[derive(Debug, Clone)]
pub struct Gauge<'a> {
    block: Option<Block<'a>>,
    ratio: f64,
    label: Option<Span<'a>>,
    use_unicode: bool,
    style: Style,
    gauge_style: Style,
}

impl<'a> Default for Gauge<'a> {
    fn default() -> Gauge<'a> {
        Gauge {
            block: None,
            ratio: 0.0,
            label: None,
            use_unicode: false,
            style: Style::default(),
            gauge_style: Style::default(),
        }
    }
}

impl<'a> Gauge<'a> {
    pub fn block(mut self, block: Block<'a>) -> Gauge<'a> {
        self.block = Some(block);
        self
    }

    pub fn percent(mut self, percent: u16) -> Gauge<'a> {
        assert!(
            percent <= 100,
            "Percentage should be between 0 and 100 inclusively."
        );
        self.ratio = f64::from(percent) / 100.0;
        self
    }

    /// Sets ratio ([0.0, 1.0]) directly.
    pub fn ratio(mut self, ratio: f64) -> Gauge<'a> {
        assert!(
            ratio <= 1.0 && ratio >= 0.0,
            "Ratio should be between 0 and 1 inclusively."
        );
        self.ratio = ratio;
        self
    }

    pub fn label<T>(mut self, label: T) -> Gauge<'a>
    where
        T: Into<Span<'a>>,
    {
        self.label = Some(label.into());
        self
    }

    pub fn style(mut self, style: Style) -> Gauge<'a> {
        self.style = style;
        self
    }

    pub fn gauge_style(mut self, style: Style) -> Gauge<'a> {
        self.gauge_style = style;
        self
    }

    pub fn use_unicode(mut self, unicode: bool) -> Gauge<'a> {
        self.use_unicode = unicode;
        self
    }
}

impl<'a> Widget for Gauge<'a> {
    fn render(mut self, area: Rect, buf: &mut Buffer) {
        buf.set_style(area, self.style);
        let gauge_area = match self.block.take() {
            Some(b) => {
                let inner_area = b.inner(area);
                b.render(area, buf);
                inner_area
            }
            None => area,
        };
        buf.set_style(gauge_area, self.gauge_style);
        if gauge_area.height < 1 {
            return;
        }

        let center = gauge_area.height / 2 + gauge_area.top();
        let width = f64::from(gauge_area.width) * self.ratio;
        //go to regular rounding behavior if we're not using unicode blocks
        let end = gauge_area.left()
            + if self.use_unicode {
                width.floor() as u16
            } else {
                width.round() as u16
            };
        // Label
        let ratio = self.ratio;
        let label = self
            .label
            .unwrap_or_else(|| Span::from(format!("{}%", (ratio * 100.0).round())));
        for y in gauge_area.top()..gauge_area.bottom() {
            // Gauge
            for x in gauge_area.left()..end {
                buf.get_mut(x, y).set_symbol(" ");
            }

            //set unicode block
            if self.use_unicode && self.ratio < 1.0 {
                buf.get_mut(end, y)
                    .set_symbol(get_unicode_block(width % 1.0));
            }

            let mut color_end = end;

            if y == center {
                let label_width = label.width() as u16;
                let middle = (gauge_area.width - label_width) / 2 + gauge_area.left();
                buf.set_span(middle, y, &label, gauge_area.right() - middle);
                if self.use_unicode && end >= middle && end < middle + label_width {
                    color_end = gauge_area.left() + (width.round() as u16); //set color on the label to the rounded gauge level
                }
            }

            // Fix colors
            for x in gauge_area.left()..color_end {
                buf.get_mut(x, y)
                    .set_fg(self.gauge_style.bg.unwrap_or(Color::Reset))
                    .set_bg(self.gauge_style.fg.unwrap_or(Color::Reset));
            }
        }
    }
}

fn get_unicode_block<'a>(frac: f64) -> &'a str {
    match (frac * 8.0).round() as u16 {
        //get how many eighths the fraction is closest to
        1 => symbols::block::ONE_EIGHTH,
        2 => symbols::block::ONE_QUARTER,
        3 => symbols::block::THREE_EIGHTHS,
        4 => symbols::block::HALF,
        5 => symbols::block::FIVE_EIGHTHS,
        6 => symbols::block::THREE_QUARTERS,
        7 => symbols::block::SEVEN_EIGHTHS,
        8 => symbols::block::FULL,
        _ => " ",
    }
}

/// A compact widget to display a task progress over a single line.
///
/// # Examples:
///
/// ```
/// # use tui::widgets::{Widget, LineGauge, Block, Borders};
/// # use tui::style::{Style, Color, Modifier};
/// # use tui::symbols;
/// LineGauge::default()
///     .block(Block::default().borders(Borders::ALL).title("Progress"))
///     .gauge_style(Style::default().fg(Color::White).bg(Color::Black).add_modifier(Modifier::BOLD))
///     .line_set(symbols::line::THICK)
///     .ratio(0.4);
/// ```
pub struct LineGauge<'a> {
    block: Option<Block<'a>>,
    ratio: f64,
    label: Option<Spans<'a>>,
    line_set: symbols::line::Set,
    style: Style,
    gauge_style: Style,
}

impl<'a> Default for LineGauge<'a> {
    fn default() -> Self {
        Self {
            block: None,
            ratio: 0.0,
            label: None,
            style: Style::default(),
            line_set: symbols::line::NORMAL,
            gauge_style: Style::default(),
        }
    }
}

impl<'a> LineGauge<'a> {
    pub fn block(mut self, block: Block<'a>) -> Self {
        self.block = Some(block);
        self
    }

    pub fn ratio(mut self, ratio: f64) -> Self {
        assert!(
            ratio <= 1.0 && ratio >= 0.0,
            "Ratio should be between 0 and 1 inclusively."
        );
        self.ratio = ratio;
        self
    }

    pub fn line_set(mut self, set: symbols::line::Set) -> Self {
        self.line_set = set;
        self
    }

    pub fn label<T>(mut self, label: T) -> Self
    where
        T: Into<Spans<'a>>,
    {
        self.label = Some(label.into());
        self
    }

    pub fn style(mut self, style: Style) -> Self {
        self.style = style;
        self
    }

    pub fn gauge_style(mut self, style: Style) -> Self {
        self.gauge_style = style;
        self
    }
}

impl<'a> Widget for LineGauge<'a> {
    fn render(mut self, area: Rect, buf: &mut Buffer) {
        buf.set_style(area, self.style);
        let gauge_area = match self.block.take() {
            Some(b) => {
                let inner_area = b.inner(area);
                b.render(area, buf);
                inner_area
            }
            None => area,
        };

        if gauge_area.height < 1 {
            return;
        }

        let ratio = self.ratio;
        let label = self
            .label
            .unwrap_or_else(move || Spans::from(format!("{:.0}%", ratio * 100.0)));
        let (col, row) = buf.set_spans(
            gauge_area.left(),
            gauge_area.top(),
            &label,
            gauge_area.width,
        );
        let start = col + 1;
        if start >= gauge_area.right() {
            return;
        }

        let end = start
            + (f64::from(gauge_area.right().saturating_sub(start)) * self.ratio).floor() as u16;
        for col in start..end {
            buf.get_mut(col, row)
                .set_symbol(self.line_set.horizontal)
                .set_style(Style {
                    fg: self.gauge_style.fg,
                    bg: None,
                    add_modifier: self.gauge_style.add_modifier,
                    sub_modifier: self.gauge_style.sub_modifier,
                });
        }
        for col in end..gauge_area.right() {
            buf.get_mut(col, row)
                .set_symbol(self.line_set.horizontal)
                .set_style(Style {
                    fg: self.gauge_style.bg,
                    bg: None,
                    add_modifier: self.gauge_style.add_modifier,
                    sub_modifier: self.gauge_style.sub_modifier,
                });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[should_panic]
    fn gauge_invalid_percentage() {
        Gauge::default().percent(110);
    }

    #[test]
    #[should_panic]
    fn gauge_invalid_ratio_upper_bound() {
        Gauge::default().ratio(1.1);
    }

    #[test]
    #[should_panic]
    fn gauge_invalid_ratio_lower_bound() {
        Gauge::default().ratio(-0.5);
    }
}
