use std::{
    fmt::Display,
    io::{Result, Write},
};

use termcolor::{Buffer, BufferWriter, Color, ColorSpec, WriteColor};

use crate::{
    buffers::Buffers,
    row::Dimension as RowDimension,
    style::{Style, StyleStruct},
    utils::display_width,
};

/// Concrete cell of a table
pub struct CellStruct {
    data: Vec<String>,
    format: CellFormat,
    style: StyleStruct,
}

impl CellStruct {
    /// Used to horizontally justify contents of a cell
    pub fn justify(mut self, justify: Justify) -> CellStruct {
        self.format.justify = justify;
        self
    }

    /// Used to vertically align the contents of a cell
    pub fn align(mut self, align: Align) -> CellStruct {
        self.format.align = align;
        self
    }

    /// Used to add padding to the contents of a cell
    pub fn padding(mut self, padding: Padding) -> CellStruct {
        self.format.padding = padding;
        self
    }

    fn color_spec(&self) -> ColorSpec {
        self.style.color_spec()
    }

    /// Returns the minimum dimensions required by the cell
    pub(crate) fn required_dimension(&self) -> Dimension {
        let height = self.data.len() + self.format.padding.top + self.format.padding.bottom;
        let width = self
            .data
            .iter()
            .map(|x| display_width(x))
            .max()
            .unwrap_or_default()
            + self.format.padding.left
            + self.format.padding.right;

        Dimension { width, height }
    }

    fn buffer(
        &self,
        writer: &BufferWriter,
        available_dimension: Dimension,
        required_dimension: Dimension,
        data: &str,
    ) -> Result<Buffer> {
        let empty_chars = match self.format.justify {
            Justify::Left => self.format.padding.left,
            Justify::Right => {
                (available_dimension.width - required_dimension.width) + self.format.padding.left
            }
            Justify::Center => {
                ((available_dimension.width - required_dimension.width) / 2)
                    + self.format.padding.left
            }
        };

        let mut buffer = writer.buffer();
        buffer.set_color(&self.color_spec())?;

        for _ in 0..empty_chars {
            write!(buffer, " ")?;
        }

        write!(buffer, "{}", data)?;

        for _ in 0..(available_dimension.width - (display_width(data) + empty_chars)) {
            write!(buffer, " ")?;
        }

        Ok(buffer)
    }

    fn top_blank_lines(
        &self,
        available_dimension: Dimension,
        required_dimension: Dimension,
    ) -> usize {
        match self.format.align {
            Align::Top => self.format.padding.top,
            Align::Bottom => {
                (available_dimension.height - required_dimension.height) + self.format.padding.top
            }
            Align::Center => {
                ((available_dimension.height - required_dimension.height) / 2)
                    + self.format.padding.top
            }
        }
    }
}

impl Buffers for CellStruct {
    type Dimension = Dimension;

    type Buffers = Vec<Buffer>;

    fn buffers(
        &self,
        writer: &BufferWriter,
        available_dimension: Self::Dimension,
    ) -> Result<Self::Buffers> {
        let required_dimension = self.required_dimension();

        assert!(
            available_dimension >= required_dimension,
            "Available dimensions for a cell are smaller than required. Please create an issue in https://github.com/devashishdxt/cli-table"
        );

        let top_blank_lines = self.top_blank_lines(available_dimension, required_dimension);
        let mut buffers = Vec::with_capacity(available_dimension.height);

        for _ in 0..top_blank_lines {
            buffers.push(self.buffer(writer, available_dimension, required_dimension, "")?);
        }

        for line in self.data.clone().iter() {
            buffers.push(self.buffer(writer, available_dimension, required_dimension, line)?);
        }

        for _ in 0..(available_dimension.height - (self.data.len() + top_blank_lines)) {
            buffers.push(self.buffer(writer, available_dimension, required_dimension, "")?);
        }

        Ok(buffers)
    }
}

/// Trait to convert raw types into cells
pub trait Cell {
    /// Converts raw type to cell of a table
    fn cell(self) -> CellStruct;
}

impl<T> Cell for T
where
    T: Display,
{
    fn cell(self) -> CellStruct {
        let data = self.to_string().lines().map(ToString::to_string).collect();

        CellStruct {
            data,
            format: Default::default(),
            style: Default::default(),
        }
    }
}

impl Cell for CellStruct {
    fn cell(self) -> CellStruct {
        self
    }
}

impl Style for CellStruct {
    fn foreground_color(mut self, foreground_color: Option<Color>) -> Self {
        self.style = self.style.foreground_color(foreground_color);
        self
    }

    fn background_color(mut self, background_color: Option<Color>) -> Self {
        self.style = self.style.background_color(background_color);
        self
    }

    fn bold(mut self, bold: bool) -> Self {
        self.style = self.style.bold(bold);
        self
    }

    fn underline(mut self, underline: bool) -> Self {
        self.style = self.style.underline(underline);
        self
    }

    fn italic(mut self, italic: bool) -> Self {
        self.style = self.style.italic(italic);
        self
    }

    fn intense(mut self, intense: bool) -> Self {
        self.style = self.style.intense(intense);
        self
    }

    fn dimmed(mut self, dimmed: bool) -> Self {
        self.style = self.style.dimmed(dimmed);
        self
    }
}

/// Struct for configuring a cell's format
#[derive(Debug, Clone, Copy, Default)]
struct CellFormat {
    justify: Justify,
    align: Align,
    padding: Padding,
}

/// Used to horizontally justify contents of a cell
#[derive(Debug, Clone, Copy)]
pub enum Justify {
    /// Justifies contents to left
    Left,
    /// Justifies contents to right
    Right,
    /// Justifies contents to center
    Center,
}

impl Default for Justify {
    fn default() -> Self {
        Self::Left
    }
}

/// Used to vertically align contents of a cell
#[derive(Debug, Clone, Copy)]
pub enum Align {
    /// Aligns contents to top
    Top,
    /// Aligns contents to bottom
    Bottom,
    /// Aligns contents to center
    Center,
}

impl Default for Align {
    fn default() -> Self {
        Self::Top
    }
}

/// Used to add padding to the contents of a cell
#[derive(Debug, Clone, Copy, Default)]
pub struct Padding {
    /// Left padding
    pub(crate) left: usize,
    /// Right padding
    pub(crate) right: usize,
    /// Top padding
    pub(crate) top: usize,
    /// Bottom padding
    pub(crate) bottom: usize,
}

impl Padding {
    /// Creates a new builder for padding
    pub fn builder() -> PaddingBuilder {
        Default::default()
    }
}

/// Builder for padding
#[derive(Debug, Default)]
pub struct PaddingBuilder(Padding);

impl PaddingBuilder {
    /// Sets left padding of a cell
    pub fn left(mut self, left_padding: usize) -> Self {
        self.0.left = left_padding;
        self
    }

    /// Sets right padding of a cell
    pub fn right(mut self, right_padding: usize) -> Self {
        self.0.right = right_padding;
        self
    }

    /// Sets top padding of a cell
    pub fn top(mut self, top_padding: usize) -> Self {
        self.0.top = top_padding;
        self
    }

    /// Sets bottom padding of a cell
    pub fn bottom(mut self, bottom_padding: usize) -> Self {
        self.0.bottom = bottom_padding;
        self
    }

    /// Build padding
    pub fn build(self) -> Padding {
        self.0
    }
}

/// Dimensions of a cell
#[derive(Debug, Copy, Clone, Ord, PartialOrd, Eq, PartialEq)]
pub(crate) struct Dimension {
    /// Width of a cell
    pub(crate) width: usize,
    /// Height of a cell
    pub(crate) height: usize,
}

impl From<RowDimension> for Vec<Dimension> {
    fn from(row_dimension: RowDimension) -> Self {
        let height = row_dimension.height;

        row_dimension
            .widths
            .into_iter()
            .map(|width| Dimension { width, height })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_into_cell() {
        let cell = "Hello".cell();
        assert_eq!(1, cell.data.len());
        assert_eq!("Hello", cell.data[0]);
    }
}
