//! This crate provides an implementation of
//! [elastic tabstops](http://nickgravgaard.com/elastictabstops/index.html).
//! It is a minimal port of Go's
//! [tabwriter](http://golang.org/pkg/text/tabwriter/) package.
//! Namely, its main mode of operation is to wrap a `Writer` and implement
//! elastic tabstops for the text written to the wrapped `Writer`.
//!
//! This package is also bundled with a program, `tabwriter`,
//! that exposes this functionality at the command line.
//!
//! Here's an example that shows basic alignment:
//!
//! ```rust
//! use std::io::Write;
//! use tabwriter::TabWriter;
//!
//! let mut tw = TabWriter::new(vec![]);
//! write!(&mut tw, "
//! Bruce Springsteen\tBorn to Run
//! Bob Seger\tNight Moves
//! Metallica\tBlack
//! The Boss\tDarkness on the Edge of Town
//! ").unwrap();
//! tw.flush().unwrap();
//!
//! let written = String::from_utf8(tw.into_inner().unwrap()).unwrap();
//! assert_eq!(&*written, "
//! Bruce Springsteen  Born to Run
//! Bob Seger          Night Moves
//! Metallica          Black
//! The Boss           Darkness on the Edge of Town
//! ");
//! ```
//!
//! Note that `flush` **must** be called or else `TabWriter` may never write
//! anything. This is because elastic tabstops requires knowing about future
//! lines in order to align output. More precisely, all text considered in a
//! single alignment must fit into memory.
//!
//! Here's another example that demonstrates how *only* contiguous columns
//! are aligned:
//!
//! ```rust
//! use std::io::Write;
//! use tabwriter::TabWriter;
//!
//! let mut tw = TabWriter::new(vec![]).padding(1);
//! write!(&mut tw, "
//!fn foobar() {{
//!    let mut x = 1+1;\t// addition
//!    x += 1;\t// increment in place
//!    let y = x * x * x * x;\t// multiply!
//!
//!    y += 1;\t// this is another group
//!    y += 2 * 2;\t// that is separately aligned
//!}}
//!").unwrap();
//! tw.flush().unwrap();
//!
//! let written = String::from_utf8(tw.into_inner().unwrap()).unwrap();
//! assert_eq!(&*written, "
//!fn foobar() {
//!    let mut x = 1+1;       // addition
//!    x += 1;                // increment in place
//!    let y = x * x * x * x; // multiply!
//!
//!    y += 1;     // this is another group
//!    y += 2 * 2; // that is separately aligned
//!}
//!");
//! ```

#![deny(missing_docs)]

use std::cmp;
use std::error;
use std::fmt;
use std::io::{self, Write};
use std::iter;
use std::mem;
use std::str;

#[cfg(test)]
mod test;

/// TabWriter wraps an arbitrary writer and aligns tabbed output.
///
/// Elastic tabstops work by aligning *contiguous* tabbed delimited fields
/// known as *column blocks*. When a line appears that breaks all contiguous
/// blocks, all buffered output will be flushed to the underlying writer.
/// Otherwise, output will stay buffered until `flush` is explicitly called.
#[derive(Debug)]
pub struct TabWriter<W> {
    w: W,
    buf: io::Cursor<Vec<u8>>,
    lines: Vec<Vec<Cell>>,
    curcell: Cell,
    minwidth: usize,
    padding: usize,
    alignment: Alignment,
}

/// `Alignment` represents how a `TabWriter` should align text within its cell.
#[derive(Debug)]
pub enum Alignment {
    /// Text should be aligned with the left edge of the cell
    Left,
    /// Text should be centered within the cell
    Center,
    /// Text should be aligned with the right edge of the cell
    Right,
}

#[derive(Debug)]
struct Cell {
    start: usize, // offset into TabWriter.buf
    width: usize, // in characters
    size: usize,  // in bytes
}

impl<W: io::Write> TabWriter<W> {
    /// Create a new `TabWriter` from an existing `Writer`.
    ///
    /// All output written to `Writer` is passed through `TabWriter`.
    /// Contiguous column blocks indicated by tabs are aligned.
    ///
    /// Note that `flush` must be called to guarantee that `TabWriter` will
    /// write to the given writer.
    pub fn new(w: W) -> TabWriter<W> {
        TabWriter {
            w: w,
            buf: io::Cursor::new(Vec::with_capacity(1024)),
            lines: vec![vec![]],
            curcell: Cell::new(0),
            minwidth: 2,
            padding: 2,
            alignment: Alignment::Left,
        }
    }

    /// Set the minimum width of each column. That is, all columns will have
    /// *at least* the size given here. If a column is smaller than `minwidth`,
    /// then it is padded with spaces.
    ///
    /// The default minimum width is `2`.
    pub fn minwidth(mut self, minwidth: usize) -> TabWriter<W> {
        self.minwidth = minwidth;
        self
    }

    /// Set the padding between columns. All columns will be separated by
    /// *at least* the number of spaces indicated by `padding`. If `padding`
    /// is zero, then columns may run up against each other without any
    /// separation.
    ///
    /// The default padding is `2`.
    pub fn padding(mut self, padding: usize) -> TabWriter<W> {
        self.padding = padding;
        self
    }

    /// Set the alignment of text within cells. This will effect future flushes.
    ///
    /// The default alignment is `Alignment::Left`.
    pub fn alignment(mut self, alignment: Alignment) -> TabWriter<W> {
        self.alignment = alignment;
        self
    }

    /// Unwraps this `TabWriter`, returning the underlying writer.
    ///
    /// This internal buffer is flushed before returning the writer. If the
    /// flush fails, then an error is returned.
    pub fn into_inner(mut self) -> Result<W, IntoInnerError<TabWriter<W>>> {
        match self.flush() {
            Ok(()) => Ok(self.w),
            Err(err) => Err(IntoInnerError(self, err)),
        }
    }

    /// Resets the state of the aligner. Once the aligner is reset, all future
    /// writes will start producing a new alignment.
    fn reset(&mut self) {
        self.buf = io::Cursor::new(Vec::with_capacity(1024));
        self.lines = vec![vec![]];
        self.curcell = Cell::new(0);
    }

    /// Adds the bytes received into the buffer and updates the size of
    /// the current cell.
    fn add_bytes(&mut self, bytes: &[u8]) {
        self.curcell.size += bytes.len();
        let _ = self.buf.write_all(bytes); // cannot fail
    }

    /// Ends the current cell, updates the UTF8 width of the cell and starts
    /// a fresh cell.
    fn term_curcell(&mut self) {
        let mut curcell = Cell::new(self.buf.position() as usize);
        mem::swap(&mut self.curcell, &mut curcell);

        curcell.update_width(&self.buf.get_ref());
        self.curline_mut().push(curcell);
    }

    /// Return a view of the current line of cells.
    fn curline(&mut self) -> &[Cell] {
        let i = self.lines.len() - 1;
        &*self.lines[i]
    }

    /// Return a mutable view of the current line of cells.
    fn curline_mut(&mut self) -> &mut Vec<Cell> {
        let i = self.lines.len() - 1;
        &mut self.lines[i]
    }
}

impl Cell {
    fn new(start: usize) -> Cell {
        Cell { start: start, width: 0, size: 0 }
    }

    fn update_width(&mut self, buf: &[u8]) {
        let end = self.start + self.size;
        self.width = display_columns(&buf[self.start..end]);
    }
}

impl<W: io::Write> io::Write for TabWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut lastterm = 0usize;
        for (i, &c) in buf.iter().enumerate() {
            match c {
                b'\t' | b'\n' => {
                    self.add_bytes(&buf[lastterm..i]);
                    self.term_curcell();
                    lastterm = i + 1;
                    if c == b'\n' {
                        let ncells = self.curline().len();
                        self.lines.push(vec![]);
                        // Having a single cell means that *all* previous
                        // columns have been broken, so we should just flush.
                        if ncells == 1 {
                            self.flush()?;
                        }
                    }
                }
                _ => {}
            }
        }
        self.add_bytes(&buf[lastterm..]);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        if self.curcell.size > 0 {
            self.term_curcell();
        }
        let widths = cell_widths(&self.lines, self.minwidth);

        // This is a trick to avoid allocating padding for every cell.
        // Just allocate the most we'll ever need and borrow from it.
        let biggest_width = widths
            .iter()
            .map(|ws| ws.iter().map(|&w| w).max().unwrap_or(0))
            .max()
            .unwrap_or(0);
        let padding: String =
            iter::repeat(' ').take(biggest_width + self.padding).collect();

        let mut first = true;
        for (line, widths) in self.lines.iter().zip(widths.iter()) {
            if !first {
                self.w.write_all(b"\n")?;
            } else {
                first = false
            }
            for (i, cell) in line.iter().enumerate() {
                let bytes =
                    &self.buf.get_ref()[cell.start..cell.start + cell.size];
                if i >= widths.len() {
                    // There is no width for the last column
                    assert_eq!(i, line.len() - 1);
                    self.w.write_all(bytes)?;
                } else {
                    assert!(widths[i] >= cell.width);
                    let extra_space = widths[i] - cell.width;
                    let (left_spaces, mut right_spaces) = match self.alignment
                    {
                        Alignment::Left => (0, extra_space),
                        Alignment::Right => (extra_space, 0),
                        Alignment::Center => {
                            (extra_space / 2, extra_space - extra_space / 2)
                        }
                    };
                    right_spaces += self.padding;
                    write!(&mut self.w, "{}", &padding[0..left_spaces])?;
                    self.w.write_all(bytes)?;
                    write!(&mut self.w, "{}", &padding[0..right_spaces])?;
                }
            }
        }

        self.reset();
        Ok(())
    }
}

/// An error returned by `into_inner`.
///
/// This combines the error that happened while flushing the buffer with the
/// `TabWriter` itself.
pub struct IntoInnerError<W>(W, io::Error);

impl<W> IntoInnerError<W> {
    /// Returns the error which caused the `into_error()` call to fail.
    pub fn error(&self) -> &io::Error {
        &self.1
    }

    /// Returns the `TabWriter` instance which generated the error.
    pub fn into_inner(self) -> W {
        self.0
    }
}

impl<W> fmt::Debug for IntoInnerError<W> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.error().fmt(f)
    }
}

impl<W> fmt::Display for IntoInnerError<W> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.error().fmt(f)
    }
}

impl<W: ::std::any::Any> error::Error for IntoInnerError<W> {
    fn description(&self) -> &str {
        self.error().description()
    }

    fn cause(&self) -> Option<&dyn error::Error> {
        Some(self.error())
    }
}

fn cell_widths(lines: &Vec<Vec<Cell>>, minwidth: usize) -> Vec<Vec<usize>> {
    // Naively, this algorithm looks like it could be O(n^2m) where `n` is
    // the number of lines and `m` is the number of contiguous columns.
    //
    // However, I claim that it is actually O(nm). That is, the width for
    // every contiguous column is computed exactly once.
    let mut ws: Vec<_> = (0..lines.len()).map(|_| vec![]).collect();
    for (i, iline) in lines.iter().enumerate() {
        if iline.is_empty() {
            continue;
        }
        for col in ws[i].len()..(iline.len() - 1) {
            let mut width = minwidth;
            let mut contig_count = 0;
            for line in lines[i..].iter() {
                if col + 1 >= line.len() {
                    // ignores last column
                    break;
                }
                contig_count += 1;
                width = cmp::max(width, line[col].width);
            }
            assert!(contig_count >= 1);
            for j in i..(i + contig_count) {
                ws[j].push(width);
            }
        }
    }
    ws
}

#[cfg(not(feature = "ansi_formatting"))]
fn display_columns(bytes: &[u8]) -> usize {
    use unicode_width::UnicodeWidthChar;

    // If we have a Unicode string, then attempt to guess the number of
    // *display* columns used.
    match str::from_utf8(bytes) {
        Err(_) => bytes.len(),
        Ok(s) => s
            .chars()
            .map(|c| UnicodeWidthChar::width(c).unwrap_or(0))
            .fold(0, |sum, width| sum + width),
    }
}

#[cfg(feature = "ansi_formatting")]
fn display_columns(bytes: &[u8]) -> usize {
    use unicode_width::UnicodeWidthChar;

    // If we have a Unicode string, then attempt to guess the number of
    // *display* columns used.
    match str::from_utf8(bytes) {
        Err(_) => bytes.len(),
        Ok(s) => strip_formatting(s)
            .chars()
            .map(|c| UnicodeWidthChar::width(c).unwrap_or(0))
            .fold(0, |sum, width| sum + width),
    }
}

#[cfg(feature = "ansi_formatting")]
fn strip_formatting<'t>(input: &'t str) -> std::borrow::Cow<'t, str> {
    use regex::Regex;

    lazy_static::lazy_static! {
        static ref RE: Regex = Regex::new(r#"\x1B\[.+?m"#).unwrap();
    }
    RE.replace_all(input, "")
}
