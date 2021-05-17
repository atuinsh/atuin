use std::io::Result;

use termcolor::{Buffer, BufferWriter, ColorSpec};

use crate::{
    buffers::Buffers,
    cell::{Cell, CellStruct, Dimension as CellDimension},
    table::{Dimension as TableDimension, TableFormat},
    utils::{print_char, print_vertical_line, println_str, transpose},
};

/// Concrete row of a table
pub struct RowStruct {
    pub(crate) cells: Vec<CellStruct>,
}

impl RowStruct {
    pub(crate) fn required_dimension(&self) -> Dimension {
        let mut widths = Vec::with_capacity(self.cells.len());
        let mut height = 0;

        for cell in self.cells.iter() {
            let cell_dimension = cell.required_dimension();

            widths.push(cell_dimension.width);

            height = std::cmp::max(cell_dimension.height, height);
        }

        Dimension { widths, height }
    }

    pub(crate) fn print_writer(
        &self,
        writer: &BufferWriter,
        dimension: Dimension,
        format: &TableFormat,
        color_spec: &ColorSpec,
    ) -> Result<()> {
        let buffers = self.buffers(&writer, dimension)?;

        for line in buffers.into_iter() {
            print_vertical_line(&writer, format.border.left.as_ref(), &color_spec)?;

            let mut line_buffers = line.into_iter().peekable();

            while let Some(buffer) = line_buffers.next() {
                print_char(&writer, ' ', &color_spec)?;
                writer.print(&buffer)?;
                print_char(&writer, ' ', &color_spec)?;

                match line_buffers.peek() {
                    Some(_) => {
                        print_vertical_line(&writer, format.separator.column.as_ref(), &color_spec)?
                    }
                    None => {
                        print_vertical_line(&writer, format.border.right.as_ref(), &color_spec)?
                    }
                }
            }

            println_str(&writer, "", &color_spec)?;
        }

        Ok(())
    }
}

impl Buffers for RowStruct {
    type Dimension = Dimension;

    type Buffers = Vec<Vec<Buffer>>;

    fn buffers(
        &self,
        writer: &BufferWriter,
        available_dimension: Self::Dimension,
    ) -> Result<Self::Buffers> {
        let available_cell_dimensions: Vec<CellDimension> = available_dimension.into();

        let cell_buffers = self
            .cells
            .iter()
            .zip(available_cell_dimensions.into_iter())
            .map(|(cell, available_dimension)| cell.buffers(writer, available_dimension))
            .collect::<Result<Vec<Vec<Buffer>>>>()?;
        Ok(transpose(cell_buffers))
    }
}

/// Trait to convert raw types into rows
pub trait Row {
    /// Converts raw type to rows of a table
    fn row(self) -> RowStruct;
}

impl<T, C> Row for T
where
    T: IntoIterator<Item = C>,
    C: Cell,
{
    fn row(self) -> RowStruct {
        let cells = self.into_iter().map(|cell| cell.cell()).collect();
        RowStruct { cells }
    }
}

impl Row for RowStruct {
    fn row(self) -> RowStruct {
        self
    }
}

/// Dimensions of a row
#[derive(Debug, Default, Clone, Eq, PartialEq)]
pub(crate) struct Dimension {
    /// Widths of each cell of row
    pub(crate) widths: Vec<usize>,
    /// Height of row
    pub(crate) height: usize,
}

impl From<TableDimension> for Vec<Dimension> {
    fn from(table_dimension: TableDimension) -> Self {
        let heights = table_dimension.heights;
        let widths = table_dimension.widths;

        heights
            .into_iter()
            .map(|height| Dimension {
                widths: widths.clone(),
                height,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use crate::style::Style;

    use super::*;

    #[test]
    fn test_into_row_with_style() {
        let row = vec!["Hello".cell().bold(true), "World".cell()].row();
        assert_eq!(2, row.cells.len());
    }

    #[test]
    fn test_into_row() {
        let row = &["Hello", "World"].row();
        assert_eq!(2, row.cells.len());
    }
}
