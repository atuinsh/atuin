use std::cell::RefCell;
use std::cmp::{max, min};
use std::collections::HashMap;
use std::rc::Rc;

use cassowary::strength::{MEDIUM, REQUIRED, WEAK};
use cassowary::WeightedRelation::*;
use cassowary::{Constraint as CassowaryConstraint, Expression, Solver, Variable};

#[derive(Debug, Hash, Clone, Copy, PartialEq, Eq)]
pub enum Corner {
    TopLeft,
    TopRight,
    BottomRight,
    BottomLeft,
}

#[derive(Debug, Hash, Clone, PartialEq, Eq)]
pub enum Direction {
    Horizontal,
    Vertical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Constraint {
    // TODO: enforce range 0 - 100
    Percentage(u16),
    Ratio(u32, u32),
    Length(u16),
    Max(u16),
    Min(u16),
}

impl Constraint {
    pub fn apply(&self, length: u16) -> u16 {
        match *self {
            Constraint::Percentage(p) => length * p / 100,
            Constraint::Ratio(num, den) => {
                let r = num * u32::from(length) / den;
                r as u16
            }
            Constraint::Length(l) => length.min(l),
            Constraint::Max(m) => length.min(m),
            Constraint::Min(m) => length.max(m),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Margin {
    pub vertical: u16,
    pub horizontal: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Alignment {
    Left,
    Center,
    Right,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Layout {
    direction: Direction,
    margin: Margin,
    constraints: Vec<Constraint>,
    /// Whether the last chunk of the computed layout should be expanded to fill the available
    /// space.
    expand_to_fill: bool,
}

type Cache = HashMap<(Rect, Layout), Rc<[Rect]>>;
thread_local! {
    static LAYOUT_CACHE: RefCell<Cache> = RefCell::new(HashMap::new());
}

impl Default for Layout {
    fn default() -> Layout {
        Layout {
            direction: Direction::Vertical,
            margin: Margin {
                horizontal: 0,
                vertical: 0,
            },
            constraints: Vec::new(),
            expand_to_fill: true,
        }
    }
}

impl Layout {
    pub fn constraints<C>(mut self, constraints: C) -> Layout
    where
        C: Into<Vec<Constraint>>,
    {
        self.constraints = constraints.into();
        self
    }

    pub fn margin(mut self, margin: u16) -> Layout {
        self.margin = Margin {
            horizontal: margin,
            vertical: margin,
        };
        self
    }

    pub fn horizontal_margin(mut self, horizontal: u16) -> Layout {
        self.margin.horizontal = horizontal;
        self
    }

    pub fn vertical_margin(mut self, vertical: u16) -> Layout {
        self.margin.vertical = vertical;
        self
    }

    pub fn direction(mut self, direction: Direction) -> Layout {
        self.direction = direction;
        self
    }

    pub(crate) fn expand_to_fill(mut self, expand_to_fill: bool) -> Layout {
        self.expand_to_fill = expand_to_fill;
        self
    }

    /// Wrapper function around the cassowary-rs solver to be able to split a given
    /// area into smaller ones based on the preferred widths or heights and the direction.
    ///
    /// # Examples
    /// ```
    /// # use ratatui::layout::{Rect, Constraint, Direction, Layout};
    /// let chunks = Layout::default()
    ///     .direction(Direction::Vertical)
    ///     .constraints([Constraint::Length(5), Constraint::Min(0)].as_ref())
    ///     .split(Rect {
    ///         x: 2,
    ///         y: 2,
    ///         width: 10,
    ///         height: 10,
    ///     });
    /// assert_eq!(
    ///     chunks[..],
    ///     [
    ///         Rect {
    ///             x: 2,
    ///             y: 2,
    ///             width: 10,
    ///             height: 5
    ///         },
    ///         Rect {
    ///             x: 2,
    ///             y: 7,
    ///             width: 10,
    ///             height: 5
    ///         }
    ///     ]
    /// );
    ///
    /// let chunks = Layout::default()
    ///     .direction(Direction::Horizontal)
    ///     .constraints([Constraint::Ratio(1, 3), Constraint::Ratio(2, 3)].as_ref())
    ///     .split(Rect {
    ///         x: 0,
    ///         y: 0,
    ///         width: 9,
    ///         height: 2,
    ///     });
    /// assert_eq!(
    ///     chunks[..],
    ///     [
    ///         Rect {
    ///             x: 0,
    ///             y: 0,
    ///             width: 3,
    ///             height: 2
    ///         },
    ///         Rect {
    ///             x: 3,
    ///             y: 0,
    ///             width: 6,
    ///             height: 2
    ///         }
    ///     ]
    /// );
    /// ```
    pub fn split(&self, area: Rect) -> Rc<[Rect]> {
        // TODO: Maybe use a fixed size cache ?
        LAYOUT_CACHE.with(|c| {
            c.borrow_mut()
                .entry((area, self.clone()))
                .or_insert_with(|| split(area, self))
                .clone()
        })
    }
}

fn split(area: Rect, layout: &Layout) -> Rc<[Rect]> {
    let mut solver = Solver::new();
    let mut vars: HashMap<Variable, (usize, usize)> = HashMap::new();
    let elements = layout
        .constraints
        .iter()
        .map(|_| Element::new())
        .collect::<Vec<Element>>();
    let mut res = layout
        .constraints
        .iter()
        .map(|_| Rect::default())
        .collect::<Rc<[Rect]>>();

    let mut results = Rc::get_mut(&mut res).expect("newly created Rc should have no shared refs");

    let dest_area = area.inner(&layout.margin);
    for (i, e) in elements.iter().enumerate() {
        vars.insert(e.x, (i, 0));
        vars.insert(e.y, (i, 1));
        vars.insert(e.width, (i, 2));
        vars.insert(e.height, (i, 3));
    }
    let mut ccs: Vec<CassowaryConstraint> =
        Vec::with_capacity(elements.len() * 4 + layout.constraints.len() * 6);
    for elt in &elements {
        ccs.push(elt.width | GE(REQUIRED) | 0f64);
        ccs.push(elt.height | GE(REQUIRED) | 0f64);
        ccs.push(elt.left() | GE(REQUIRED) | f64::from(dest_area.left()));
        ccs.push(elt.top() | GE(REQUIRED) | f64::from(dest_area.top()));
        ccs.push(elt.right() | LE(REQUIRED) | f64::from(dest_area.right()));
        ccs.push(elt.bottom() | LE(REQUIRED) | f64::from(dest_area.bottom()));
    }
    if let Some(first) = elements.first() {
        ccs.push(match layout.direction {
            Direction::Horizontal => first.left() | EQ(REQUIRED) | f64::from(dest_area.left()),
            Direction::Vertical => first.top() | EQ(REQUIRED) | f64::from(dest_area.top()),
        });
    }
    if layout.expand_to_fill {
        if let Some(last) = elements.last() {
            ccs.push(match layout.direction {
                Direction::Horizontal => last.right() | EQ(REQUIRED) | f64::from(dest_area.right()),
                Direction::Vertical => last.bottom() | EQ(REQUIRED) | f64::from(dest_area.bottom()),
            });
        }
    }
    match layout.direction {
        Direction::Horizontal => {
            for pair in elements.windows(2) {
                ccs.push((pair[0].x + pair[0].width) | EQ(REQUIRED) | pair[1].x);
            }
            for (i, size) in layout.constraints.iter().enumerate() {
                ccs.push(elements[i].y | EQ(REQUIRED) | f64::from(dest_area.y));
                ccs.push(elements[i].height | EQ(REQUIRED) | f64::from(dest_area.height));
                ccs.push(match *size {
                    Constraint::Length(v) => elements[i].width | EQ(MEDIUM) | f64::from(v),
                    Constraint::Percentage(v) => {
                        elements[i].width | EQ(MEDIUM) | (f64::from(v * dest_area.width) / 100.0)
                    }
                    Constraint::Ratio(n, d) => {
                        elements[i].width
                            | EQ(MEDIUM)
                            | (f64::from(dest_area.width) * f64::from(n) / f64::from(d))
                    }
                    Constraint::Min(v) => elements[i].width | GE(MEDIUM) | f64::from(v),
                    Constraint::Max(v) => elements[i].width | LE(MEDIUM) | f64::from(v),
                });

                match *size {
                    Constraint::Min(v) => {
                        ccs.push(elements[i].width | EQ(WEAK) | f64::from(v));
                    }
                    Constraint::Max(v) => {
                        ccs.push(elements[i].width | EQ(WEAK) | f64::from(v));
                    }
                    _ => {}
                }
            }
        }
        Direction::Vertical => {
            for pair in elements.windows(2) {
                ccs.push((pair[0].y + pair[0].height) | EQ(REQUIRED) | pair[1].y);
            }
            for (i, size) in layout.constraints.iter().enumerate() {
                ccs.push(elements[i].x | EQ(REQUIRED) | f64::from(dest_area.x));
                ccs.push(elements[i].width | EQ(REQUIRED) | f64::from(dest_area.width));
                ccs.push(match *size {
                    Constraint::Length(v) => elements[i].height | EQ(MEDIUM) | f64::from(v),
                    Constraint::Percentage(v) => {
                        elements[i].height | EQ(MEDIUM) | (f64::from(v * dest_area.height) / 100.0)
                    }
                    Constraint::Ratio(n, d) => {
                        elements[i].height
                            | EQ(MEDIUM)
                            | (f64::from(dest_area.height) * f64::from(n) / f64::from(d))
                    }
                    Constraint::Min(v) => elements[i].height | GE(MEDIUM) | f64::from(v),
                    Constraint::Max(v) => elements[i].height | LE(MEDIUM) | f64::from(v),
                });

                match *size {
                    Constraint::Min(v) => {
                        ccs.push(elements[i].height | EQ(WEAK) | f64::from(v));
                    }
                    Constraint::Max(v) => {
                        ccs.push(elements[i].height | EQ(WEAK) | f64::from(v));
                    }
                    _ => {}
                }
            }
        }
    }
    solver.add_constraints(&ccs).unwrap();
    for &(var, value) in solver.fetch_changes() {
        let (index, attr) = vars[&var];
        let value = if value.is_sign_negative() {
            0
        } else {
            value as u16
        };
        match attr {
            0 => {
                results[index].x = value;
            }
            1 => {
                results[index].y = value;
            }
            2 => {
                results[index].width = value;
            }
            3 => {
                results[index].height = value;
            }
            _ => {}
        }
    }

    if layout.expand_to_fill {
        // Fix imprecision by extending the last item a bit if necessary
        if let Some(last) = results.last_mut() {
            match layout.direction {
                Direction::Vertical => {
                    last.height = dest_area.bottom() - last.y;
                }
                Direction::Horizontal => {
                    last.width = dest_area.right() - last.x;
                }
            }
        }
    }
    res
}

/// A container used by the solver inside split
struct Element {
    x: Variable,
    y: Variable,
    width: Variable,
    height: Variable,
}

impl Element {
    fn new() -> Element {
        Element {
            x: Variable::new(),
            y: Variable::new(),
            width: Variable::new(),
            height: Variable::new(),
        }
    }

    fn left(&self) -> Variable {
        self.x
    }

    fn top(&self) -> Variable {
        self.y
    }

    fn right(&self) -> Expression {
        self.x + self.width
    }

    fn bottom(&self) -> Expression {
        self.y + self.height
    }
}

/// A simple rectangle used in the computation of the layout and to give widgets a hint about the
/// area they are supposed to render to.
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, Default)]
pub struct Rect {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

impl Rect {
    /// Creates a new rect, with width and height limited to keep the area under max u16.
    /// If clipped, aspect ratio will be preserved.
    pub fn new(x: u16, y: u16, width: u16, height: u16) -> Rect {
        let max_area = u16::max_value();
        let (clipped_width, clipped_height) =
            if u32::from(width) * u32::from(height) > u32::from(max_area) {
                let aspect_ratio = f64::from(width) / f64::from(height);
                let max_area_f = f64::from(max_area);
                let height_f = (max_area_f / aspect_ratio).sqrt();
                let width_f = height_f * aspect_ratio;
                (width_f as u16, height_f as u16)
            } else {
                (width, height)
            };
        Rect {
            x,
            y,
            width: clipped_width,
            height: clipped_height,
        }
    }

    pub fn area(self) -> u16 {
        self.width * self.height
    }

    pub fn left(self) -> u16 {
        self.x
    }

    pub fn right(self) -> u16 {
        self.x.saturating_add(self.width)
    }

    pub fn top(self) -> u16 {
        self.y
    }

    pub fn bottom(self) -> u16 {
        self.y.saturating_add(self.height)
    }

    pub fn inner(self, margin: &Margin) -> Rect {
        if self.width < 2 * margin.horizontal || self.height < 2 * margin.vertical {
            Rect::default()
        } else {
            Rect {
                x: self.x + margin.horizontal,
                y: self.y + margin.vertical,
                width: self.width - 2 * margin.horizontal,
                height: self.height - 2 * margin.vertical,
            }
        }
    }

    pub fn union(self, other: Rect) -> Rect {
        let x1 = min(self.x, other.x);
        let y1 = min(self.y, other.y);
        let x2 = max(self.x + self.width, other.x + other.width);
        let y2 = max(self.y + self.height, other.y + other.height);
        Rect {
            x: x1,
            y: y1,
            width: x2 - x1,
            height: y2 - y1,
        }
    }

    pub fn intersection(self, other: Rect) -> Rect {
        let x1 = max(self.x, other.x);
        let y1 = max(self.y, other.y);
        let x2 = min(self.x + self.width, other.x + other.width);
        let y2 = min(self.y + self.height, other.y + other.height);
        Rect {
            x: x1,
            y: y1,
            width: x2 - x1,
            height: y2 - y1,
        }
    }

    pub fn intersects(self, other: Rect) -> bool {
        self.x < other.x + other.width
            && self.x + self.width > other.x
            && self.y < other.y + other.height
            && self.y + self.height > other.y
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vertical_split_by_height() {
        let target = Rect {
            x: 2,
            y: 2,
            width: 10,
            height: 10,
        };

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints(
                [
                    Constraint::Percentage(10),
                    Constraint::Max(5),
                    Constraint::Min(1),
                ]
                .as_ref(),
            )
            .split(target);

        assert_eq!(target.height, chunks.iter().map(|r| r.height).sum::<u16>());
        chunks.windows(2).for_each(|w| assert!(w[0].y <= w[1].y));
    }

    #[test]
    fn test_rect_size_truncation() {
        for width in 256u16..300u16 {
            for height in 256u16..300u16 {
                let rect = Rect::new(0, 0, width, height);
                rect.area(); // Should not panic.
                assert!(rect.width < width || rect.height < height);
                // The target dimensions are rounded down so the math will not be too precise
                // but let's make sure the ratios don't diverge crazily.
                assert!(
                    (f64::from(rect.width) / f64::from(rect.height)
                        - f64::from(width) / f64::from(height))
                    .abs()
                        < 1.0
                )
            }
        }

        // One dimension below 255, one above. Area above max u16.
        let width = 900;
        let height = 100;
        let rect = Rect::new(0, 0, width, height);
        assert_ne!(rect.width, 900);
        assert_ne!(rect.height, 100);
        assert!(rect.width < width || rect.height < height);
    }

    #[test]
    fn test_rect_size_preservation() {
        for width in 0..256u16 {
            for height in 0..256u16 {
                let rect = Rect::new(0, 0, width, height);
                rect.area(); // Should not panic.
                assert_eq!(rect.width, width);
                assert_eq!(rect.height, height);
            }
        }

        // One dimension below 255, one above. Area below max u16.
        let rect = Rect::new(0, 0, 300, 100);
        assert_eq!(rect.width, 300);
        assert_eq!(rect.height, 100);
    }
}
