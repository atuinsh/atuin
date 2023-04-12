use std::{borrow::Cow, cmp::max};

use unicode_width::UnicodeWidthStr;

use crate::ratatui::layout::Alignment;
use crate::ratatui::{
    buffer::Buffer,
    layout::{Constraint, Rect},
    style::{Color, Style},
    symbols,
    text::{Span, Spans},
    widgets::{
        canvas::{Canvas, Line, Points},
        Block, Borders, Widget,
    },
};

/// An X or Y axis for the chart widget
#[derive(Debug, Clone)]
pub struct Axis<'a> {
    /// Title displayed next to axis end
    title: Option<Spans<'a>>,
    /// Bounds for the axis (all data points outside these limits will not be represented)
    bounds: [f64; 2],
    /// A list of labels to put to the left or below the axis
    labels: Option<Vec<Span<'a>>>,
    /// The style used to draw the axis itself
    style: Style,
    /// The alignment of the labels of the Axis
    labels_alignment: Alignment,
}

impl<'a> Default for Axis<'a> {
    fn default() -> Axis<'a> {
        Axis {
            title: None,
            bounds: [0.0, 0.0],
            labels: None,
            style: Default::default(),
            labels_alignment: Alignment::Left,
        }
    }
}

impl<'a> Axis<'a> {
    pub fn title<T>(mut self, title: T) -> Axis<'a>
    where
        T: Into<Spans<'a>>,
    {
        self.title = Some(title.into());
        self
    }

    #[deprecated(
        since = "0.10.0",
        note = "You should use styling capabilities of `text::Spans` given as argument of the `title` method to apply styling to the title."
    )]
    pub fn title_style(mut self, style: Style) -> Axis<'a> {
        if let Some(t) = self.title {
            let title = String::from(t);
            self.title = Some(Spans::from(Span::styled(title, style)));
        }
        self
    }

    pub fn bounds(mut self, bounds: [f64; 2]) -> Axis<'a> {
        self.bounds = bounds;
        self
    }

    pub fn labels(mut self, labels: Vec<Span<'a>>) -> Axis<'a> {
        self.labels = Some(labels);
        self
    }

    pub fn style(mut self, style: Style) -> Axis<'a> {
        self.style = style;
        self
    }

    /// Defines the alignment of the labels of the axis.
    /// The alignment behaves differently based on the axis:
    /// - Y-Axis: The labels are aligned within the area on the left of the axis
    /// - X-Axis: The first X-axis label is aligned relative to the Y-axis
    pub fn labels_alignment(mut self, alignment: Alignment) -> Axis<'a> {
        self.labels_alignment = alignment;
        self
    }
}

/// Used to determine which style of graphing to use
#[derive(Debug, Clone, Copy)]
pub enum GraphType {
    /// Draw each point
    Scatter,
    /// Draw each point and lines between each point using the same marker
    Line,
}

/// A group of data points
#[derive(Debug, Clone)]
pub struct Dataset<'a> {
    /// Name of the dataset (used in the legend if shown)
    name: Cow<'a, str>,
    /// A reference to the actual data
    data: &'a [(f64, f64)],
    /// Symbol used for each points of this dataset
    marker: symbols::Marker,
    /// Determines graph type used for drawing points
    graph_type: GraphType,
    /// Style used to plot this dataset
    style: Style,
}

impl<'a> Default for Dataset<'a> {
    fn default() -> Dataset<'a> {
        Dataset {
            name: Cow::from(""),
            data: &[],
            marker: symbols::Marker::Dot,
            graph_type: GraphType::Scatter,
            style: Style::default(),
        }
    }
}

impl<'a> Dataset<'a> {
    pub fn name<S>(mut self, name: S) -> Dataset<'a>
    where
        S: Into<Cow<'a, str>>,
    {
        self.name = name.into();
        self
    }

    pub fn data(mut self, data: &'a [(f64, f64)]) -> Dataset<'a> {
        self.data = data;
        self
    }

    pub fn marker(mut self, marker: symbols::Marker) -> Dataset<'a> {
        self.marker = marker;
        self
    }

    pub fn graph_type(mut self, graph_type: GraphType) -> Dataset<'a> {
        self.graph_type = graph_type;
        self
    }

    pub fn style(mut self, style: Style) -> Dataset<'a> {
        self.style = style;
        self
    }
}

/// A container that holds all the infos about where to display each elements of the chart (axis,
/// labels, legend, ...).
#[derive(Debug, Clone, PartialEq, Default)]
struct ChartLayout {
    /// Location of the title of the x axis
    title_x: Option<(u16, u16)>,
    /// Location of the title of the y axis
    title_y: Option<(u16, u16)>,
    /// Location of the first label of the x axis
    label_x: Option<u16>,
    /// Location of the first label of the y axis
    label_y: Option<u16>,
    /// Y coordinate of the horizontal axis
    axis_x: Option<u16>,
    /// X coordinate of the vertical axis
    axis_y: Option<u16>,
    /// Area of the legend
    legend_area: Option<Rect>,
    /// Area of the graph
    graph_area: Rect,
}

/// A widget to plot one or more dataset in a cartesian coordinate system
///
/// # Examples
///
/// ```
/// # use ratatui::symbols;
/// # use ratatui::widgets::{Block, Borders, Chart, Axis, Dataset, GraphType};
/// # use ratatui::style::{Style, Color};
/// # use ratatui::text::Span;
/// let datasets = vec![
///     Dataset::default()
///         .name("data1")
///         .marker(symbols::Marker::Dot)
///         .graph_type(GraphType::Scatter)
///         .style(Style::default().fg(Color::Cyan))
///         .data(&[(0.0, 5.0), (1.0, 6.0), (1.5, 6.434)]),
///     Dataset::default()
///         .name("data2")
///         .marker(symbols::Marker::Braille)
///         .graph_type(GraphType::Line)
///         .style(Style::default().fg(Color::Magenta))
///         .data(&[(4.0, 5.0), (5.0, 8.0), (7.66, 13.5)]),
/// ];
/// Chart::new(datasets)
///     .block(Block::default().title("Chart"))
///     .x_axis(Axis::default()
///         .title(Span::styled("X Axis", Style::default().fg(Color::Red)))
///         .style(Style::default().fg(Color::White))
///         .bounds([0.0, 10.0])
///         .labels(["0.0", "5.0", "10.0"].iter().cloned().map(Span::from).collect()))
///     .y_axis(Axis::default()
///         .title(Span::styled("Y Axis", Style::default().fg(Color::Red)))
///         .style(Style::default().fg(Color::White))
///         .bounds([0.0, 10.0])
///         .labels(["0.0", "5.0", "10.0"].iter().cloned().map(Span::from).collect()));
/// ```
#[derive(Debug, Clone)]
pub struct Chart<'a> {
    /// A block to display around the widget eventually
    block: Option<Block<'a>>,
    /// The horizontal axis
    x_axis: Axis<'a>,
    /// The vertical axis
    y_axis: Axis<'a>,
    /// A reference to the datasets
    datasets: Vec<Dataset<'a>>,
    /// The widget base style
    style: Style,
    /// Constraints used to determine whether the legend should be shown or not
    hidden_legend_constraints: (Constraint, Constraint),
}

impl<'a> Chart<'a> {
    pub fn new(datasets: Vec<Dataset<'a>>) -> Chart<'a> {
        Chart {
            block: None,
            x_axis: Axis::default(),
            y_axis: Axis::default(),
            style: Default::default(),
            datasets,
            hidden_legend_constraints: (Constraint::Ratio(1, 4), Constraint::Ratio(1, 4)),
        }
    }

    pub fn block(mut self, block: Block<'a>) -> Chart<'a> {
        self.block = Some(block);
        self
    }

    pub fn style(mut self, style: Style) -> Chart<'a> {
        self.style = style;
        self
    }

    pub fn x_axis(mut self, axis: Axis<'a>) -> Chart<'a> {
        self.x_axis = axis;
        self
    }

    pub fn y_axis(mut self, axis: Axis<'a>) -> Chart<'a> {
        self.y_axis = axis;
        self
    }

    /// Set the constraints used to determine whether the legend should be shown or not.
    ///
    /// # Examples
    ///
    /// ```
    /// # use ratatui::widgets::Chart;
    /// # use ratatui::layout::Constraint;
    /// let constraints = (
    ///     Constraint::Ratio(1, 3),
    ///     Constraint::Ratio(1, 4)
    /// );
    /// // Hide the legend when either its width is greater than 33% of the total widget width
    /// // or if its height is greater than 25% of the total widget height.
    /// let _chart: Chart = Chart::new(vec![])
    ///     .hidden_legend_constraints(constraints);
    /// ```
    pub fn hidden_legend_constraints(mut self, constraints: (Constraint, Constraint)) -> Chart<'a> {
        self.hidden_legend_constraints = constraints;
        self
    }

    /// Compute the internal layout of the chart given the area. If the area is too small some
    /// elements may be automatically hidden
    fn layout(&self, area: Rect) -> ChartLayout {
        let mut layout = ChartLayout::default();
        if area.height == 0 || area.width == 0 {
            return layout;
        }
        let mut x = area.left();
        let mut y = area.bottom() - 1;

        if self.x_axis.labels.is_some() && y > area.top() {
            layout.label_x = Some(y);
            y -= 1;
        }

        layout.label_y = self.y_axis.labels.as_ref().and(Some(x));
        x += self.max_width_of_labels_left_of_y_axis(area, self.y_axis.labels.is_some());

        if self.x_axis.labels.is_some() && y > area.top() {
            layout.axis_x = Some(y);
            y -= 1;
        }

        if self.y_axis.labels.is_some() && x + 1 < area.right() {
            layout.axis_y = Some(x);
            x += 1;
        }

        if x < area.right() && y > 1 {
            layout.graph_area = Rect::new(x, area.top(), area.right() - x, y - area.top() + 1);
        }

        if let Some(ref title) = self.x_axis.title {
            let w = title.width() as u16;
            if w < layout.graph_area.width && layout.graph_area.height > 2 {
                layout.title_x = Some((x + layout.graph_area.width - w, y));
            }
        }

        if let Some(ref title) = self.y_axis.title {
            let w = title.width() as u16;
            if w + 1 < layout.graph_area.width && layout.graph_area.height > 2 {
                layout.title_y = Some((x, area.top()));
            }
        }

        if let Some(inner_width) = self.datasets.iter().map(|d| d.name.width() as u16).max() {
            let legend_width = inner_width + 2;
            let legend_height = self.datasets.len() as u16 + 2;
            let max_legend_width = self
                .hidden_legend_constraints
                .0
                .apply(layout.graph_area.width);
            let max_legend_height = self
                .hidden_legend_constraints
                .1
                .apply(layout.graph_area.height);
            if inner_width > 0
                && legend_width < max_legend_width
                && legend_height < max_legend_height
            {
                layout.legend_area = Some(Rect::new(
                    layout.graph_area.right() - legend_width,
                    layout.graph_area.top(),
                    legend_width,
                    legend_height,
                ));
            }
        }
        layout
    }

    fn max_width_of_labels_left_of_y_axis(&self, area: Rect, has_y_axis: bool) -> u16 {
        let mut max_width = self
            .y_axis
            .labels
            .as_ref()
            .map(|l| l.iter().map(Span::width).max().unwrap_or_default() as u16)
            .unwrap_or_default();

        if let Some(first_x_label) = self.x_axis.labels.as_ref().and_then(|labels| labels.get(0)) {
            let first_label_width = first_x_label.content.width() as u16;
            let width_left_of_y_axis = match self.x_axis.labels_alignment {
                Alignment::Left => {
                    // The last character of the label should be below the Y-Axis when it exists, not on its left
                    let y_axis_offset = if has_y_axis { 1 } else { 0 };
                    first_label_width.saturating_sub(y_axis_offset)
                }
                Alignment::Center => first_label_width / 2,
                Alignment::Right => 0,
            };
            max_width = max(max_width, width_left_of_y_axis);
        }
        // labels of y axis and first label of x axis can take at most 1/3rd of the total width
        max_width.min(area.width / 3)
    }

    fn render_x_labels(
        &mut self,
        buf: &mut Buffer,
        layout: &ChartLayout,
        chart_area: Rect,
        graph_area: Rect,
    ) {
        let y = match layout.label_x {
            Some(y) => y,
            None => return,
        };
        let labels = self.x_axis.labels.as_ref().unwrap();
        let labels_len = labels.len() as u16;
        if labels_len < 2 {
            return;
        }

        let width_between_ticks = graph_area.width / labels_len;

        let label_area = self.first_x_label_area(
            y,
            labels.first().unwrap().width() as u16,
            width_between_ticks,
            chart_area,
            graph_area,
        );

        let label_alignment = match self.x_axis.labels_alignment {
            Alignment::Left => Alignment::Right,
            Alignment::Center => Alignment::Center,
            Alignment::Right => Alignment::Left,
        };

        Self::render_label(buf, labels.first().unwrap(), label_area, label_alignment);

        for (i, label) in labels[1..labels.len() - 1].iter().enumerate() {
            // We add 1 to x (and width-1 below) to leave at least one space before each intermediate labels
            let x = graph_area.left() + (i + 1) as u16 * width_between_ticks + 1;
            let label_area = Rect::new(x, y, width_between_ticks.saturating_sub(1), 1);

            Self::render_label(buf, label, label_area, Alignment::Center);
        }

        let x = graph_area.right() - width_between_ticks;
        let label_area = Rect::new(x, y, width_between_ticks, 1);
        // The last label should be aligned Right to be at the edge of the graph area
        Self::render_label(buf, labels.last().unwrap(), label_area, Alignment::Right);
    }

    fn first_x_label_area(
        &self,
        y: u16,
        label_width: u16,
        max_width_after_y_axis: u16,
        chart_area: Rect,
        graph_area: Rect,
    ) -> Rect {
        let (min_x, max_x) = match self.x_axis.labels_alignment {
            Alignment::Left => (chart_area.left(), graph_area.left()),
            Alignment::Center => (
                chart_area.left(),
                graph_area.left() + max_width_after_y_axis.min(label_width),
            ),
            Alignment::Right => (
                graph_area.left().saturating_sub(1),
                graph_area.left() + max_width_after_y_axis,
            ),
        };

        Rect::new(min_x, y, max_x - min_x, 1)
    }

    fn render_label(buf: &mut Buffer, label: &Span, label_area: Rect, alignment: Alignment) {
        let label_width = label.width() as u16;
        let bounded_label_width = label_area.width.min(label_width);

        let x = match alignment {
            Alignment::Left => label_area.left(),
            Alignment::Center => label_area.left() + label_area.width / 2 - bounded_label_width / 2,
            Alignment::Right => label_area.right() - bounded_label_width,
        };

        buf.set_span(x, label_area.top(), label, bounded_label_width);
    }

    fn render_y_labels(
        &mut self,
        buf: &mut Buffer,
        layout: &ChartLayout,
        chart_area: Rect,
        graph_area: Rect,
    ) {
        let x = match layout.label_y {
            Some(x) => x,
            None => return,
        };
        let labels = self.y_axis.labels.as_ref().unwrap();
        let labels_len = labels.len() as u16;
        for (i, label) in labels.iter().enumerate() {
            let dy = i as u16 * (graph_area.height - 1) / (labels_len - 1);
            if dy < graph_area.bottom() {
                let label_area = Rect::new(
                    x,
                    graph_area.bottom().saturating_sub(1) - dy,
                    (graph_area.left() - chart_area.left()).saturating_sub(1),
                    1,
                );
                Self::render_label(buf, label, label_area, self.y_axis.labels_alignment);
            }
        }
    }
}

impl<'a> Widget for Chart<'a> {
    fn render(mut self, area: Rect, buf: &mut Buffer) {
        if area.area() == 0 {
            return;
        }
        buf.set_style(area, self.style);
        // Sample the style of the entire widget. This sample will be used to reset the style of
        // the cells that are part of the components put on top of the grah area (i.e legend and
        // axis names).
        let original_style = buf.get(area.left(), area.top()).style();

        let chart_area = match self.block.take() {
            Some(b) => {
                let inner_area = b.inner(area);
                b.render(area, buf);
                inner_area
            }
            None => area,
        };

        let layout = self.layout(chart_area);
        let graph_area = layout.graph_area;
        if graph_area.width < 1 || graph_area.height < 1 {
            return;
        }

        self.render_x_labels(buf, &layout, chart_area, graph_area);
        self.render_y_labels(buf, &layout, chart_area, graph_area);

        if let Some(y) = layout.axis_x {
            for x in graph_area.left()..graph_area.right() {
                buf.get_mut(x, y)
                    .set_symbol(symbols::line::HORIZONTAL)
                    .set_style(self.x_axis.style);
            }
        }

        if let Some(x) = layout.axis_y {
            for y in graph_area.top()..graph_area.bottom() {
                buf.get_mut(x, y)
                    .set_symbol(symbols::line::VERTICAL)
                    .set_style(self.y_axis.style);
            }
        }

        if let Some(y) = layout.axis_x {
            if let Some(x) = layout.axis_y {
                buf.get_mut(x, y)
                    .set_symbol(symbols::line::BOTTOM_LEFT)
                    .set_style(self.x_axis.style);
            }
        }

        for dataset in &self.datasets {
            Canvas::default()
                .background_color(self.style.bg.unwrap_or(Color::Reset))
                .x_bounds(self.x_axis.bounds)
                .y_bounds(self.y_axis.bounds)
                .marker(dataset.marker)
                .paint(|ctx| {
                    ctx.draw(&Points {
                        coords: dataset.data,
                        color: dataset.style.fg.unwrap_or(Color::Reset),
                    });
                    if let GraphType::Line = dataset.graph_type {
                        for data in dataset.data.windows(2) {
                            ctx.draw(&Line {
                                x1: data[0].0,
                                y1: data[0].1,
                                x2: data[1].0,
                                y2: data[1].1,
                                color: dataset.style.fg.unwrap_or(Color::Reset),
                            })
                        }
                    }
                })
                .render(graph_area, buf);
        }

        if let Some(legend_area) = layout.legend_area {
            buf.set_style(legend_area, original_style);
            Block::default()
                .borders(Borders::ALL)
                .render(legend_area, buf);
            for (i, dataset) in self.datasets.iter().enumerate() {
                buf.set_string(
                    legend_area.x + 1,
                    legend_area.y + 1 + i as u16,
                    &dataset.name,
                    dataset.style,
                );
            }
        }

        if let Some((x, y)) = layout.title_x {
            let title = self.x_axis.title.unwrap();
            let width = graph_area.right().saturating_sub(x);
            buf.set_style(
                Rect {
                    x,
                    y,
                    width,
                    height: 1,
                },
                original_style,
            );
            buf.set_spans(x, y, &title, width);
        }

        if let Some((x, y)) = layout.title_y {
            let title = self.y_axis.title.unwrap();
            let width = graph_area.right().saturating_sub(x);
            buf.set_style(
                Rect {
                    x,
                    y,
                    width,
                    height: 1,
                },
                original_style,
            );
            buf.set_spans(x, y, &title, width);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct LegendTestCase {
        chart_area: Rect,
        hidden_legend_constraints: (Constraint, Constraint),
        legend_area: Option<Rect>,
    }

    #[test]
    fn it_should_hide_the_legend() {
        let data = [(0.0, 5.0), (1.0, 6.0), (3.0, 7.0)];
        let cases = [
            LegendTestCase {
                chart_area: Rect::new(0, 0, 100, 100),
                hidden_legend_constraints: (Constraint::Ratio(1, 4), Constraint::Ratio(1, 4)),
                legend_area: Some(Rect::new(88, 0, 12, 12)),
            },
            LegendTestCase {
                chart_area: Rect::new(0, 0, 100, 100),
                hidden_legend_constraints: (Constraint::Ratio(1, 10), Constraint::Ratio(1, 4)),
                legend_area: None,
            },
        ];
        for case in &cases {
            let datasets = (0..10)
                .map(|i| {
                    let name = format!("Dataset #{}", i);
                    Dataset::default().name(name).data(&data)
                })
                .collect::<Vec<_>>();
            let chart = Chart::new(datasets)
                .x_axis(Axis::default().title("X axis"))
                .y_axis(Axis::default().title("Y axis"))
                .hidden_legend_constraints(case.hidden_legend_constraints);
            let layout = chart.layout(case.chart_area);
            assert_eq!(layout.legend_area, case.legend_area);
        }
    }
}
