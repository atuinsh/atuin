use crate::{
    style::Color,
    widgets::canvas::{Line, Painter, Shape},
};

/// Shape to draw a rectangle from a `Rect` with the given color
#[derive(Debug, Clone)]
pub struct Rectangle {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub color: Color,
}

impl Shape for Rectangle {
    fn draw(&self, painter: &mut Painter) {
        let lines: [Line; 4] = [
            Line {
                x1: self.x,
                y1: self.y,
                x2: self.x,
                y2: self.y + self.height,
                color: self.color,
            },
            Line {
                x1: self.x,
                y1: self.y + self.height,
                x2: self.x + self.width,
                y2: self.y + self.height,
                color: self.color,
            },
            Line {
                x1: self.x + self.width,
                y1: self.y,
                x2: self.x + self.width,
                y2: self.y + self.height,
                color: self.color,
            },
            Line {
                x1: self.x,
                y1: self.y,
                x2: self.x + self.width,
                y2: self.y,
                color: self.color,
            },
        ];
        for line in &lines {
            line.draw(painter);
        }
    }
}
