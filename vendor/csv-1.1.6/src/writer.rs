use std::fs::File;
use std::io;
use std::path::Path;
use std::result;

use csv_core::{
    self, WriteResult, Writer as CoreWriter,
    WriterBuilder as CoreWriterBuilder,
};
use serde::Serialize;

use crate::byte_record::ByteRecord;
use crate::error::{Error, ErrorKind, IntoInnerError, Result};
use crate::serializer::{serialize, serialize_header};
use crate::{QuoteStyle, Terminator};

/// Builds a CSV writer with various configuration knobs.
///
/// This builder can be used to tweak the field delimiter, record terminator
/// and more. Once a CSV `Writer` is built, its configuration cannot be
/// changed.
#[derive(Debug)]
pub struct WriterBuilder {
    builder: CoreWriterBuilder,
    capacity: usize,
    flexible: bool,
    has_headers: bool,
}

impl Default for WriterBuilder {
    fn default() -> WriterBuilder {
        WriterBuilder {
            builder: CoreWriterBuilder::default(),
            capacity: 8 * (1 << 10),
            flexible: false,
            has_headers: true,
        }
    }
}

impl WriterBuilder {
    /// Create a new builder for configuring CSV writing.
    ///
    /// To convert a builder into a writer, call one of the methods starting
    /// with `from_`.
    ///
    /// # Example
    ///
    /// ```
    /// use std::error::Error;
    /// use csv::WriterBuilder;
    ///
    /// # fn main() { example().unwrap(); }
    /// fn example() -> Result<(), Box<dyn Error>> {
    ///     let mut wtr = WriterBuilder::new().from_writer(vec![]);
    ///     wtr.write_record(&["a", "b", "c"])?;
    ///     wtr.write_record(&["x", "y", "z"])?;
    ///
    ///     let data = String::from_utf8(wtr.into_inner()?)?;
    ///     assert_eq!(data, "a,b,c\nx,y,z\n");
    ///     Ok(())
    /// }
    /// ```
    pub fn new() -> WriterBuilder {
        WriterBuilder::default()
    }

    /// Build a CSV writer from this configuration that writes data to the
    /// given file path. The file is truncated if it already exists.
    ///
    /// If there was a problem opening the file at the given path, then this
    /// returns the corresponding error.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use std::error::Error;
    /// use csv::WriterBuilder;
    ///
    /// # fn main() { example().unwrap(); }
    /// fn example() -> Result<(), Box<dyn Error>> {
    ///     let mut wtr = WriterBuilder::new().from_path("foo.csv")?;
    ///     wtr.write_record(&["a", "b", "c"])?;
    ///     wtr.write_record(&["x", "y", "z"])?;
    ///     wtr.flush()?;
    ///     Ok(())
    /// }
    /// ```
    pub fn from_path<P: AsRef<Path>>(&self, path: P) -> Result<Writer<File>> {
        Ok(Writer::new(self, File::create(path)?))
    }

    /// Build a CSV writer from this configuration that writes data to `wtr`.
    ///
    /// Note that the CSV writer is buffered automatically, so you should not
    /// wrap `wtr` in a buffered writer like `io::BufWriter`.
    ///
    /// # Example
    ///
    /// ```
    /// use std::error::Error;
    /// use csv::WriterBuilder;
    ///
    /// # fn main() { example().unwrap(); }
    /// fn example() -> Result<(), Box<dyn Error>> {
    ///     let mut wtr = WriterBuilder::new().from_writer(vec![]);
    ///     wtr.write_record(&["a", "b", "c"])?;
    ///     wtr.write_record(&["x", "y", "z"])?;
    ///
    ///     let data = String::from_utf8(wtr.into_inner()?)?;
    ///     assert_eq!(data, "a,b,c\nx,y,z\n");
    ///     Ok(())
    /// }
    /// ```
    pub fn from_writer<W: io::Write>(&self, wtr: W) -> Writer<W> {
        Writer::new(self, wtr)
    }

    /// The field delimiter to use when writing CSV.
    ///
    /// The default is `b','`.
    ///
    /// # Example
    ///
    /// ```
    /// use std::error::Error;
    /// use csv::WriterBuilder;
    ///
    /// # fn main() { example().unwrap(); }
    /// fn example() -> Result<(), Box<dyn Error>> {
    ///     let mut wtr = WriterBuilder::new()
    ///         .delimiter(b';')
    ///         .from_writer(vec![]);
    ///     wtr.write_record(&["a", "b", "c"])?;
    ///     wtr.write_record(&["x", "y", "z"])?;
    ///
    ///     let data = String::from_utf8(wtr.into_inner()?)?;
    ///     assert_eq!(data, "a;b;c\nx;y;z\n");
    ///     Ok(())
    /// }
    /// ```
    pub fn delimiter(&mut self, delimiter: u8) -> &mut WriterBuilder {
        self.builder.delimiter(delimiter);
        self
    }

    /// Whether to write a header row before writing any other row.
    ///
    /// When this is enabled and the `serialize` method is used to write data
    /// with something that contains field names (i.e., a struct), then a
    /// header row is written containing the field names before any other row
    /// is written.
    ///
    /// This option has no effect when using other methods to write rows. That
    /// is, if you don't use `serialize`, then you must write your header row
    /// explicitly if you want a header row.
    ///
    /// This is enabled by default.
    ///
    /// # Example: with headers
    ///
    /// This shows how the header will be automatically written from the field
    /// names of a struct.
    ///
    /// ```
    /// use std::error::Error;
    ///
    /// use csv::WriterBuilder;
    /// use serde::Serialize;
    ///
    /// #[derive(Serialize)]
    /// struct Row<'a> {
    ///     city: &'a str,
    ///     country: &'a str,
    ///     // Serde allows us to name our headers exactly,
    ///     // even if they don't match our struct field names.
    ///     #[serde(rename = "popcount")]
    ///     population: u64,
    /// }
    ///
    /// # fn main() { example().unwrap(); }
    /// fn example() -> Result<(), Box<dyn Error>> {
    ///     let mut wtr = WriterBuilder::new().from_writer(vec![]);
    ///     wtr.serialize(Row {
    ///         city: "Boston",
    ///         country: "United States",
    ///         population: 4628910,
    ///     })?;
    ///     wtr.serialize(Row {
    ///         city: "Concord",
    ///         country: "United States",
    ///         population: 42695,
    ///     })?;
    ///
    ///     let data = String::from_utf8(wtr.into_inner()?)?;
    ///     assert_eq!(data, "\
    /// city,country,popcount
    /// Boston,United States,4628910
    /// Concord,United States,42695
    /// ");
    ///     Ok(())
    /// }
    /// ```
    ///
    /// # Example: without headers
    ///
    /// This shows that serializing things that aren't structs (in this case,
    /// a tuple struct) won't result in a header row being written. This means
    /// you usually don't need to set `has_headers(false)` unless you
    /// explicitly want to both write custom headers and serialize structs.
    ///
    /// ```
    /// use std::error::Error;
    /// use csv::WriterBuilder;
    ///
    /// # fn main() { example().unwrap(); }
    /// fn example() -> Result<(), Box<dyn Error>> {
    ///     let mut wtr = WriterBuilder::new().from_writer(vec![]);
    ///     wtr.serialize(("Boston", "United States", 4628910))?;
    ///     wtr.serialize(("Concord", "United States", 42695))?;
    ///
    ///     let data = String::from_utf8(wtr.into_inner()?)?;
    ///     assert_eq!(data, "\
    /// Boston,United States,4628910
    /// Concord,United States,42695
    /// ");
    ///     Ok(())
    /// }
    /// ```
    pub fn has_headers(&mut self, yes: bool) -> &mut WriterBuilder {
        self.has_headers = yes;
        self
    }

    /// Whether the number of fields in records is allowed to change or not.
    ///
    /// When disabled (which is the default), writing CSV data will return an
    /// error if a record is written with a number of fields different from the
    /// number of fields written in a previous record.
    ///
    /// When enabled, this error checking is turned off.
    ///
    /// # Example: writing flexible records
    ///
    /// ```
    /// use std::error::Error;
    /// use csv::WriterBuilder;
    ///
    /// # fn main() { example().unwrap(); }
    /// fn example() -> Result<(), Box<dyn Error>> {
    ///     let mut wtr = WriterBuilder::new()
    ///         .flexible(true)
    ///         .from_writer(vec![]);
    ///     wtr.write_record(&["a", "b"])?;
    ///     wtr.write_record(&["x", "y", "z"])?;
    ///
    ///     let data = String::from_utf8(wtr.into_inner()?)?;
    ///     assert_eq!(data, "a,b\nx,y,z\n");
    ///     Ok(())
    /// }
    /// ```
    ///
    /// # Example: error when `flexible` is disabled
    ///
    /// ```
    /// use std::error::Error;
    /// use csv::WriterBuilder;
    ///
    /// # fn main() { example().unwrap(); }
    /// fn example() -> Result<(), Box<dyn Error>> {
    ///     let mut wtr = WriterBuilder::new()
    ///         .flexible(false)
    ///         .from_writer(vec![]);
    ///     wtr.write_record(&["a", "b"])?;
    ///     let err = wtr.write_record(&["x", "y", "z"]).unwrap_err();
    ///     match *err.kind() {
    ///         csv::ErrorKind::UnequalLengths { expected_len, len, .. } => {
    ///             assert_eq!(expected_len, 2);
    ///             assert_eq!(len, 3);
    ///         }
    ///         ref wrong => {
    ///             panic!("expected UnequalLengths but got {:?}", wrong);
    ///         }
    ///     }
    ///     Ok(())
    /// }
    /// ```
    pub fn flexible(&mut self, yes: bool) -> &mut WriterBuilder {
        self.flexible = yes;
        self
    }

    /// The record terminator to use when writing CSV.
    ///
    /// A record terminator can be any single byte. The default is `\n`.
    ///
    /// Note that RFC 4180 specifies that record terminators should be `\r\n`.
    /// To use `\r\n`, use the special `Terminator::CRLF` value.
    ///
    /// # Example: CRLF
    ///
    /// This shows how to use RFC 4180 compliant record terminators.
    ///
    /// ```
    /// use std::error::Error;
    /// use csv::{Terminator, WriterBuilder};
    ///
    /// # fn main() { example().unwrap(); }
    /// fn example() -> Result<(), Box<dyn Error>> {
    ///     let mut wtr = WriterBuilder::new()
    ///         .terminator(Terminator::CRLF)
    ///         .from_writer(vec![]);
    ///     wtr.write_record(&["a", "b", "c"])?;
    ///     wtr.write_record(&["x", "y", "z"])?;
    ///
    ///     let data = String::from_utf8(wtr.into_inner()?)?;
    ///     assert_eq!(data, "a,b,c\r\nx,y,z\r\n");
    ///     Ok(())
    /// }
    /// ```
    pub fn terminator(&mut self, term: Terminator) -> &mut WriterBuilder {
        self.builder.terminator(term.to_core());
        self
    }

    /// The quoting style to use when writing CSV.
    ///
    /// By default, this is set to `QuoteStyle::Necessary`, which will only
    /// use quotes when they are necessary to preserve the integrity of data.
    ///
    /// Note that unless the quote style is set to `Never`, an empty field is
    /// quoted if it is the only field in a record.
    ///
    /// # Example: non-numeric quoting
    ///
    /// This shows how to quote non-numeric fields only.
    ///
    /// ```
    /// use std::error::Error;
    /// use csv::{QuoteStyle, WriterBuilder};
    ///
    /// # fn main() { example().unwrap(); }
    /// fn example() -> Result<(), Box<dyn Error>> {
    ///     let mut wtr = WriterBuilder::new()
    ///         .quote_style(QuoteStyle::NonNumeric)
    ///         .from_writer(vec![]);
    ///     wtr.write_record(&["a", "5", "c"])?;
    ///     wtr.write_record(&["3.14", "y", "z"])?;
    ///
    ///     let data = String::from_utf8(wtr.into_inner()?)?;
    ///     assert_eq!(data, "\"a\",5,\"c\"\n3.14,\"y\",\"z\"\n");
    ///     Ok(())
    /// }
    /// ```
    ///
    /// # Example: never quote
    ///
    /// This shows how the CSV writer can be made to never write quotes, even
    /// if it sacrifices the integrity of the data.
    ///
    /// ```
    /// use std::error::Error;
    /// use csv::{QuoteStyle, WriterBuilder};
    ///
    /// # fn main() { example().unwrap(); }
    /// fn example() -> Result<(), Box<dyn Error>> {
    ///     let mut wtr = WriterBuilder::new()
    ///         .quote_style(QuoteStyle::Never)
    ///         .from_writer(vec![]);
    ///     wtr.write_record(&["a", "foo\nbar", "c"])?;
    ///     wtr.write_record(&["g\"h\"i", "y", "z"])?;
    ///
    ///     let data = String::from_utf8(wtr.into_inner()?)?;
    ///     assert_eq!(data, "a,foo\nbar,c\ng\"h\"i,y,z\n");
    ///     Ok(())
    /// }
    /// ```
    pub fn quote_style(&mut self, style: QuoteStyle) -> &mut WriterBuilder {
        self.builder.quote_style(style.to_core());
        self
    }

    /// The quote character to use when writing CSV.
    ///
    /// The default is `b'"'`.
    ///
    /// # Example
    ///
    /// ```
    /// use std::error::Error;
    /// use csv::WriterBuilder;
    ///
    /// # fn main() { example().unwrap(); }
    /// fn example() -> Result<(), Box<dyn Error>> {
    ///     let mut wtr = WriterBuilder::new()
    ///         .quote(b'\'')
    ///         .from_writer(vec![]);
    ///     wtr.write_record(&["a", "foo\nbar", "c"])?;
    ///     wtr.write_record(&["g'h'i", "y\"y\"y", "z"])?;
    ///
    ///     let data = String::from_utf8(wtr.into_inner()?)?;
    ///     assert_eq!(data, "a,'foo\nbar',c\n'g''h''i',y\"y\"y,z\n");
    ///     Ok(())
    /// }
    /// ```
    pub fn quote(&mut self, quote: u8) -> &mut WriterBuilder {
        self.builder.quote(quote);
        self
    }

    /// Enable double quote escapes.
    ///
    /// This is enabled by default, but it may be disabled. When disabled,
    /// quotes in field data are escaped instead of doubled.
    ///
    /// # Example
    ///
    /// ```
    /// use std::error::Error;
    /// use csv::WriterBuilder;
    ///
    /// # fn main() { example().unwrap(); }
    /// fn example() -> Result<(), Box<dyn Error>> {
    ///     let mut wtr = WriterBuilder::new()
    ///         .double_quote(false)
    ///         .from_writer(vec![]);
    ///     wtr.write_record(&["a", "foo\"bar", "c"])?;
    ///     wtr.write_record(&["x", "y", "z"])?;
    ///
    ///     let data = String::from_utf8(wtr.into_inner()?)?;
    ///     assert_eq!(data, "a,\"foo\\\"bar\",c\nx,y,z\n");
    ///     Ok(())
    /// }
    /// ```
    pub fn double_quote(&mut self, yes: bool) -> &mut WriterBuilder {
        self.builder.double_quote(yes);
        self
    }

    /// The escape character to use when writing CSV.
    ///
    /// In some variants of CSV, quotes are escaped using a special escape
    /// character like `\` (instead of escaping quotes by doubling them).
    ///
    /// By default, writing these idiosyncratic escapes is disabled, and is
    /// only used when `double_quote` is disabled.
    ///
    /// # Example
    ///
    /// ```
    /// use std::error::Error;
    /// use csv::WriterBuilder;
    ///
    /// # fn main() { example().unwrap(); }
    /// fn example() -> Result<(), Box<dyn Error>> {
    ///     let mut wtr = WriterBuilder::new()
    ///         .double_quote(false)
    ///         .escape(b'$')
    ///         .from_writer(vec![]);
    ///     wtr.write_record(&["a", "foo\"bar", "c"])?;
    ///     wtr.write_record(&["x", "y", "z"])?;
    ///
    ///     let data = String::from_utf8(wtr.into_inner()?)?;
    ///     assert_eq!(data, "a,\"foo$\"bar\",c\nx,y,z\n");
    ///     Ok(())
    /// }
    /// ```
    pub fn escape(&mut self, escape: u8) -> &mut WriterBuilder {
        self.builder.escape(escape);
        self
    }

    /// Set the capacity (in bytes) of the internal buffer used in the CSV
    /// writer. This defaults to a reasonable setting.
    pub fn buffer_capacity(&mut self, capacity: usize) -> &mut WriterBuilder {
        self.capacity = capacity;
        self
    }
}

/// A already configured CSV writer.
///
/// A CSV writer takes as input Rust values and writes those values in a valid
/// CSV format as output.
///
/// While CSV writing is considerably easier than parsing CSV, a proper writer
/// will do a number of things for you:
///
/// 1. Quote fields when necessary.
/// 2. Check that all records have the same number of fields.
/// 3. Write records with a single empty field correctly.
/// 4. Automatically serialize normal Rust types to CSV records. When that
///    type is a struct, a header row is automatically written corresponding
///    to the fields of that struct.
/// 5. Use buffering intelligently and otherwise avoid allocation. (This means
///    that callers should not do their own buffering.)
///
/// All of the above can be configured using a
/// [`WriterBuilder`](struct.WriterBuilder.html).
/// However, a `Writer` has a couple of convenience constructors (`from_path`
/// and `from_writer`) that use the default configuration.
///
/// Note that the default configuration of a `Writer` uses `\n` for record
/// terminators instead of `\r\n` as specified by RFC 4180. Use the
/// `terminator` method on `WriterBuilder` to set the terminator to `\r\n` if
/// it's desired.
#[derive(Debug)]
pub struct Writer<W: io::Write> {
    core: CoreWriter,
    wtr: Option<W>,
    buf: Buffer,
    state: WriterState,
}

#[derive(Debug)]
struct WriterState {
    /// Whether the Serde serializer should attempt to write a header row.
    header: HeaderState,
    /// Whether inconsistent record lengths are allowed.
    flexible: bool,
    /// The number of fields writtein in the first record. This is compared
    /// with `fields_written` on all subsequent records to check for
    /// inconsistent record lengths.
    first_field_count: Option<u64>,
    /// The number of fields written in this record. This is used to report
    /// errors for inconsistent record lengths if `flexible` is disabled.
    fields_written: u64,
    /// This is set immediately before flushing the buffer and then unset
    /// immediately after flushing the buffer. This avoids flushing the buffer
    /// twice if the inner writer panics.
    panicked: bool,
}

/// HeaderState encodes a small state machine for handling header writes.
#[derive(Debug)]
enum HeaderState {
    /// Indicates that we should attempt to write a header.
    Write,
    /// Indicates that writing a header was attempt, and a header was written.
    DidWrite,
    /// Indicates that writing a header was attempted, but no headers were
    /// written or the attempt failed.
    DidNotWrite,
    /// This state is used when headers are disabled. It cannot transition
    /// to any other state.
    None,
}

/// A simple internal buffer for buffering writes.
///
/// We need this because the `csv_core` APIs want to write into a `&mut [u8]`,
/// which is not available with the `std::io::BufWriter` API.
#[derive(Debug)]
struct Buffer {
    /// The contents of the buffer.
    buf: Vec<u8>,
    /// The number of bytes written to the buffer.
    len: usize,
}

impl<W: io::Write> Drop for Writer<W> {
    fn drop(&mut self) {
        if self.wtr.is_some() && !self.state.panicked {
            let _ = self.flush();
        }
    }
}

impl Writer<File> {
    /// Build a CSV writer with a default configuration that writes data to the
    /// given file path. The file is truncated if it already exists.
    ///
    /// If there was a problem opening the file at the given path, then this
    /// returns the corresponding error.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use std::error::Error;
    /// use csv::Writer;
    ///
    /// # fn main() { example().unwrap(); }
    /// fn example() -> Result<(), Box<dyn Error>> {
    ///     let mut wtr = Writer::from_path("foo.csv")?;
    ///     wtr.write_record(&["a", "b", "c"])?;
    ///     wtr.write_record(&["x", "y", "z"])?;
    ///     wtr.flush()?;
    ///     Ok(())
    /// }
    /// ```
    pub fn from_path<P: AsRef<Path>>(path: P) -> Result<Writer<File>> {
        WriterBuilder::new().from_path(path)
    }
}

impl<W: io::Write> Writer<W> {
    fn new(builder: &WriterBuilder, wtr: W) -> Writer<W> {
        let header_state = if builder.has_headers {
            HeaderState::Write
        } else {
            HeaderState::None
        };
        Writer {
            core: builder.builder.build(),
            wtr: Some(wtr),
            buf: Buffer { buf: vec![0; builder.capacity], len: 0 },
            state: WriterState {
                header: header_state,
                flexible: builder.flexible,
                first_field_count: None,
                fields_written: 0,
                panicked: false,
            },
        }
    }

    /// Build a CSV writer with a default configuration that writes data to
    /// `wtr`.
    ///
    /// Note that the CSV writer is buffered automatically, so you should not
    /// wrap `wtr` in a buffered writer like `io::BufWriter`.
    ///
    /// # Example
    ///
    /// ```
    /// use std::error::Error;
    /// use csv::Writer;
    ///
    /// # fn main() { example().unwrap(); }
    /// fn example() -> Result<(), Box<dyn Error>> {
    ///     let mut wtr = Writer::from_writer(vec![]);
    ///     wtr.write_record(&["a", "b", "c"])?;
    ///     wtr.write_record(&["x", "y", "z"])?;
    ///
    ///     let data = String::from_utf8(wtr.into_inner()?)?;
    ///     assert_eq!(data, "a,b,c\nx,y,z\n");
    ///     Ok(())
    /// }
    /// ```
    pub fn from_writer(wtr: W) -> Writer<W> {
        WriterBuilder::new().from_writer(wtr)
    }

    /// Serialize a single record using Serde.
    ///
    /// # Example
    ///
    /// This shows how to serialize normal Rust structs as CSV records. The
    /// fields of the struct are used to write a header row automatically.
    /// (Writing the header row automatically can be disabled by building the
    /// CSV writer with a [`WriterBuilder`](struct.WriterBuilder.html) and
    /// calling the `has_headers` method.)
    ///
    /// ```
    /// use std::error::Error;
    ///
    /// use csv::Writer;
    /// use serde::Serialize;
    ///
    /// #[derive(Serialize)]
    /// struct Row<'a> {
    ///     city: &'a str,
    ///     country: &'a str,
    ///     // Serde allows us to name our headers exactly,
    ///     // even if they don't match our struct field names.
    ///     #[serde(rename = "popcount")]
    ///     population: u64,
    /// }
    ///
    /// # fn main() { example().unwrap(); }
    /// fn example() -> Result<(), Box<dyn Error>> {
    ///     let mut wtr = Writer::from_writer(vec![]);
    ///     wtr.serialize(Row {
    ///         city: "Boston",
    ///         country: "United States",
    ///         population: 4628910,
    ///     })?;
    ///     wtr.serialize(Row {
    ///         city: "Concord",
    ///         country: "United States",
    ///         population: 42695,
    ///     })?;
    ///
    ///     let data = String::from_utf8(wtr.into_inner()?)?;
    ///     assert_eq!(data, "\
    /// city,country,popcount
    /// Boston,United States,4628910
    /// Concord,United States,42695
    /// ");
    ///     Ok(())
    /// }
    /// ```
    ///
    /// # Rules
    ///
    /// The behavior of `serialize` is fairly simple:
    ///
    /// 1. Nested containers (tuples, `Vec`s, structs, etc.) are always
    ///    flattened (depth-first order).
    ///
    /// 2. If `has_headers` is `true` and the type contains field names, then
    ///    a header row is automatically generated.
    ///
    /// However, some container types cannot be serialized, and if
    /// `has_headers` is `true`, there are some additional restrictions on the
    /// types that can be serialized. See below for details.
    ///
    /// For the purpose of this section, Rust types can be divided into three
    /// categories: scalars, non-struct containers, and structs.
    ///
    /// ## Scalars
    ///
    /// Single values with no field names are written like the following. Note
    /// that some of the outputs may be quoted, according to the selected
    /// quoting style.
    ///
    /// | Name | Example Type | Example Value | Output |
    /// | ---- | ---- | ---- | ---- |
    /// | boolean | `bool` | `true` | `true` |
    /// | integers | `i8`, `i16`, `i32`, `i64`, `i128`, `u8`, `u16`, `u32`, `u64`, `u128` | `5` | `5` |
    /// | floats | `f32`, `f64` | `3.14` | `3.14` |
    /// | character | `char` | `'☃'` | `☃` |
    /// | string | `&str` | `"hi"` | `hi` |
    /// | bytes | `&[u8]` | `b"hi"[..]` | `hi` |
    /// | option | `Option` | `None` | *empty* |
    /// | option |          | `Some(5)` | `5` |
    /// | unit | `()` | `()` | *empty* |
    /// | unit struct | `struct Foo;` | `Foo` | `Foo` |
    /// | unit enum variant | `enum E { A, B }` | `E::A` | `A` |
    /// | newtype struct | `struct Foo(u8);` | `Foo(5)` | `5` |
    /// | newtype enum variant | `enum E { A(u8) }` | `E::A(5)` | `5` |
    ///
    /// Note that this table includes simple structs and enums. For example, to
    /// serialize a field from either an integer or a float type, one can do
    /// this:
    ///
    /// ```
    /// use std::error::Error;
    ///
    /// use csv::Writer;
    /// use serde::Serialize;
    ///
    /// #[derive(Serialize)]
    /// struct Row {
    ///     label: String,
    ///     value: Value,
    /// }
    ///
    /// #[derive(Serialize)]
    /// enum Value {
    ///     Integer(i64),
    ///     Float(f64),
    /// }
    ///
    /// # fn main() { example().unwrap(); }
    /// fn example() -> Result<(), Box<dyn Error>> {
    ///     let mut wtr = Writer::from_writer(vec![]);
    ///     wtr.serialize(Row {
    ///         label: "foo".to_string(),
    ///         value: Value::Integer(3),
    ///     })?;
    ///     wtr.serialize(Row {
    ///         label: "bar".to_string(),
    ///         value: Value::Float(3.14),
    ///     })?;
    ///
    ///     let data = String::from_utf8(wtr.into_inner()?)?;
    ///     assert_eq!(data, "\
    /// label,value
    /// foo,3
    /// bar,3.14
    /// ");
    ///     Ok(())
    /// }
    /// ```
    ///
    /// ## Non-Struct Containers
    ///
    /// Nested containers are flattened to their scalar components, with the
    /// exception of a few types that are not allowed:
    ///
    /// | Name | Example Type | Example Value | Output |
    /// | ---- | ---- | ---- | ---- |
    /// | sequence | `Vec<u8>` | `vec![1, 2, 3]` | `1,2,3` |
    /// | tuple | `(u8, bool)` | `(5, true)` | `5,true` |
    /// | tuple struct | `Foo(u8, bool)` | `Foo(5, true)` | `5,true` |
    /// | tuple enum variant | `enum E { A(u8, bool) }` | `E::A(5, true)` | *error* |
    /// | struct enum variant | `enum E { V { a: u8, b: bool } }` | `E::V { a: 5, b: true }` | *error* |
    /// | map | `BTreeMap<K, V>` | `BTreeMap::new()` | *error* |
    ///
    /// ## Structs
    ///
    /// Like the other containers, structs are flattened to their scalar
    /// components:
    ///
    /// | Name | Example Type | Example Value | Output |
    /// | ---- | ---- | ---- | ---- |
    /// | struct | `struct Foo { a: u8, b: bool }` | `Foo { a: 5, b: true }` | `5,true` |
    ///
    /// If `has_headers` is `false`, then there are no additional restrictions;
    /// types can be nested arbitrarily. For example:
    ///
    /// ```
    /// use std::error::Error;
    ///
    /// use csv::WriterBuilder;
    /// use serde::Serialize;
    ///
    /// #[derive(Serialize)]
    /// struct Row {
    ///     label: String,
    ///     values: Vec<f64>,
    /// }
    ///
    /// # fn main() { example().unwrap(); }
    /// fn example() -> Result<(), Box<dyn Error>> {
    ///     let mut wtr = WriterBuilder::new()
    ///         .has_headers(false)
    ///         .from_writer(vec![]);
    ///     wtr.serialize(Row {
    ///         label: "foo".to_string(),
    ///         values: vec![1.1234, 2.5678, 3.14],
    ///     })?;
    ///
    ///     let data = String::from_utf8(wtr.into_inner()?)?;
    ///     assert_eq!(data, "\
    /// foo,1.1234,2.5678,3.14
    /// ");
    ///     Ok(())
    /// }
    /// ```
    ///
    /// However, if `has_headers` were enabled in the above example, then
    /// serialization would return an error. Specifically, when `has_headers` is
    /// `true`, there are two restrictions:
    ///
    /// 1. Named field values in structs must be scalars.
    ///
    /// 2. All scalars must be named field values in structs.
    ///
    /// Other than these two restrictions, types can be nested arbitrarily.
    /// Here are a few examples:
    ///
    /// | Value | Header | Record |
    /// | ---- | ---- | ---- |
    /// | `(Foo { x: 5, y: 6 }, Bar { z: true })` | `x,y,z` | `5,6,true` |
    /// | `vec![Foo { x: 5, y: 6 }, Foo { x: 7, y: 8 }]` | `x,y,x,y` | `5,6,7,8` |
    /// | `(Foo { x: 5, y: 6 }, vec![Bar { z: Baz(true) }])` | `x,y,z` | `5,6,true` |
    /// | `Foo { x: 5, y: (6, 7) }` | *error: restriction 1* | `5,6,7` |
    /// | `(5, Foo { x: 6, y: 7 }` | *error: restriction 2* | `5,6,7` |
    /// | `(Foo { x: 5, y: 6 }, true)` | *error: restriction 2* | `5,6,true` |
    pub fn serialize<S: Serialize>(&mut self, record: S) -> Result<()> {
        if let HeaderState::Write = self.state.header {
            let wrote_header = serialize_header(self, &record)?;
            if wrote_header {
                self.write_terminator()?;
                self.state.header = HeaderState::DidWrite;
            } else {
                self.state.header = HeaderState::DidNotWrite;
            };
        }
        serialize(self, &record)?;
        self.write_terminator()?;
        Ok(())
    }

    /// Write a single record.
    ///
    /// This method accepts something that can be turned into an iterator that
    /// yields elements that can be represented by a `&[u8]`.
    ///
    /// This may be called with an empty iterator, which will cause a record
    /// terminator to be written. If no fields had been written, then a single
    /// empty field is written before the terminator.
    ///
    /// # Example
    ///
    /// ```
    /// use std::error::Error;
    /// use csv::Writer;
    ///
    /// # fn main() { example().unwrap(); }
    /// fn example() -> Result<(), Box<dyn Error>> {
    ///     let mut wtr = Writer::from_writer(vec![]);
    ///     wtr.write_record(&["a", "b", "c"])?;
    ///     wtr.write_record(&["x", "y", "z"])?;
    ///
    ///     let data = String::from_utf8(wtr.into_inner()?)?;
    ///     assert_eq!(data, "a,b,c\nx,y,z\n");
    ///     Ok(())
    /// }
    /// ```
    pub fn write_record<I, T>(&mut self, record: I) -> Result<()>
    where
        I: IntoIterator<Item = T>,
        T: AsRef<[u8]>,
    {
        for field in record.into_iter() {
            self.write_field_impl(field)?;
        }
        self.write_terminator()
    }

    /// Write a single `ByteRecord`.
    ///
    /// This method accepts a borrowed `ByteRecord` and writes its contents
    /// to the underlying writer.
    ///
    /// This is similar to `write_record` except that it specifically requires
    /// a `ByteRecord`. This permits the writer to possibly write the record
    /// more quickly than the more generic `write_record`.
    ///
    /// This may be called with an empty record, which will cause a record
    /// terminator to be written. If no fields had been written, then a single
    /// empty field is written before the terminator.
    ///
    /// # Example
    ///
    /// ```
    /// use std::error::Error;
    /// use csv::{ByteRecord, Writer};
    ///
    /// # fn main() { example().unwrap(); }
    /// fn example() -> Result<(), Box<dyn Error>> {
    ///     let mut wtr = Writer::from_writer(vec![]);
    ///     wtr.write_byte_record(&ByteRecord::from(&["a", "b", "c"][..]))?;
    ///     wtr.write_byte_record(&ByteRecord::from(&["x", "y", "z"][..]))?;
    ///
    ///     let data = String::from_utf8(wtr.into_inner()?)?;
    ///     assert_eq!(data, "a,b,c\nx,y,z\n");
    ///     Ok(())
    /// }
    /// ```
    #[inline(never)]
    pub fn write_byte_record(&mut self, record: &ByteRecord) -> Result<()> {
        if record.as_slice().is_empty() {
            return self.write_record(record);
        }
        // The idea here is to find a fast path for shuffling our record into
        // our buffer as quickly as possible. We do this because the underlying
        // "core" CSV writer does a lot of book-keeping to maintain its state
        // oriented API.
        //
        // The fast path occurs when we know our record will fit in whatever
        // space we have left in our buffer. We can actually quickly compute
        // the upper bound on the space required:
        let upper_bound =
            // The data itself plus the worst case: every byte is a quote.
            (2 * record.as_slice().len())
            // The number of field delimiters.
            + (record.len().saturating_sub(1))
            // The maximum number of quotes inserted around each field.
            + (2 * record.len())
            // The maximum number of bytes for the terminator.
            + 2;
        if self.buf.writable().len() < upper_bound {
            return self.write_record(record);
        }
        let mut first = true;
        for field in record.iter() {
            if !first {
                self.buf.writable()[0] = self.core.get_delimiter();
                self.buf.written(1);
            }
            first = false;

            if !self.core.should_quote(field) {
                self.buf.writable()[..field.len()].copy_from_slice(field);
                self.buf.written(field.len());
            } else {
                self.buf.writable()[0] = self.core.get_quote();
                self.buf.written(1);
                let (res, nin, nout) = csv_core::quote(
                    field,
                    self.buf.writable(),
                    self.core.get_quote(),
                    self.core.get_escape(),
                    self.core.get_double_quote(),
                );
                debug_assert!(res == WriteResult::InputEmpty);
                debug_assert!(nin == field.len());
                self.buf.written(nout);
                self.buf.writable()[0] = self.core.get_quote();
                self.buf.written(1);
            }
        }
        self.state.fields_written = record.len() as u64;
        self.write_terminator_into_buffer()
    }

    /// Write a single field.
    ///
    /// One should prefer using `write_record` over this method. It is provided
    /// for cases where writing a field at a time is more convenient than
    /// writing a record at a time.
    ///
    /// Note that if this API is used, `write_record` should be called with an
    /// empty iterator to write a record terminator.
    ///
    /// # Example
    ///
    /// ```
    /// use std::error::Error;
    /// use csv::Writer;
    ///
    /// # fn main() { example().unwrap(); }
    /// fn example() -> Result<(), Box<dyn Error>> {
    ///     let mut wtr = Writer::from_writer(vec![]);
    ///     wtr.write_field("a")?;
    ///     wtr.write_field("b")?;
    ///     wtr.write_field("c")?;
    ///     wtr.write_record(None::<&[u8]>)?;
    ///     wtr.write_field("x")?;
    ///     wtr.write_field("y")?;
    ///     wtr.write_field("z")?;
    ///     wtr.write_record(None::<&[u8]>)?;
    ///
    ///     let data = String::from_utf8(wtr.into_inner()?)?;
    ///     assert_eq!(data, "a,b,c\nx,y,z\n");
    ///     Ok(())
    /// }
    /// ```
    pub fn write_field<T: AsRef<[u8]>>(&mut self, field: T) -> Result<()> {
        self.write_field_impl(field)
    }

    /// Implementation of write_field.
    ///
    /// This is a separate method so we can force the compiler to inline it
    /// into write_record.
    #[inline(always)]
    fn write_field_impl<T: AsRef<[u8]>>(&mut self, field: T) -> Result<()> {
        if self.state.fields_written > 0 {
            self.write_delimiter()?;
        }
        let mut field = field.as_ref();
        loop {
            let (res, nin, nout) = self.core.field(field, self.buf.writable());
            field = &field[nin..];
            self.buf.written(nout);
            match res {
                WriteResult::InputEmpty => {
                    self.state.fields_written += 1;
                    return Ok(());
                }
                WriteResult::OutputFull => self.flush_buf()?,
            }
        }
    }

    /// Flush the contents of the internal buffer to the underlying writer.
    ///
    /// If there was a problem writing to the underlying writer, then an error
    /// is returned.
    ///
    /// Note that this also flushes the underlying writer.
    pub fn flush(&mut self) -> io::Result<()> {
        self.flush_buf()?;
        self.wtr.as_mut().unwrap().flush()?;
        Ok(())
    }

    /// Flush the contents of the internal buffer to the underlying writer,
    /// without flushing the underlying writer.
    fn flush_buf(&mut self) -> io::Result<()> {
        self.state.panicked = true;
        let result = self.wtr.as_mut().unwrap().write_all(self.buf.readable());
        self.state.panicked = false;
        result?;
        self.buf.clear();
        Ok(())
    }

    /// Flush the contents of the internal buffer and return the underlying
    /// writer.
    pub fn into_inner(
        mut self,
    ) -> result::Result<W, IntoInnerError<Writer<W>>> {
        match self.flush() {
            Ok(()) => Ok(self.wtr.take().unwrap()),
            Err(err) => Err(IntoInnerError::new(self, err)),
        }
    }

    /// Write a CSV delimiter.
    fn write_delimiter(&mut self) -> Result<()> {
        loop {
            let (res, nout) = self.core.delimiter(self.buf.writable());
            self.buf.written(nout);
            match res {
                WriteResult::InputEmpty => return Ok(()),
                WriteResult::OutputFull => self.flush_buf()?,
            }
        }
    }

    /// Write a CSV terminator.
    fn write_terminator(&mut self) -> Result<()> {
        self.check_field_count()?;
        loop {
            let (res, nout) = self.core.terminator(self.buf.writable());
            self.buf.written(nout);
            match res {
                WriteResult::InputEmpty => {
                    self.state.fields_written = 0;
                    return Ok(());
                }
                WriteResult::OutputFull => self.flush_buf()?,
            }
        }
    }

    /// Write a CSV terminator that is guaranteed to fit into the current
    /// buffer.
    #[inline(never)]
    fn write_terminator_into_buffer(&mut self) -> Result<()> {
        self.check_field_count()?;
        match self.core.get_terminator() {
            csv_core::Terminator::CRLF => {
                self.buf.writable()[0] = b'\r';
                self.buf.writable()[1] = b'\n';
                self.buf.written(2);
            }
            csv_core::Terminator::Any(b) => {
                self.buf.writable()[0] = b;
                self.buf.written(1);
            }
            _ => unreachable!(),
        }
        self.state.fields_written = 0;
        Ok(())
    }

    fn check_field_count(&mut self) -> Result<()> {
        if !self.state.flexible {
            match self.state.first_field_count {
                None => {
                    self.state.first_field_count =
                        Some(self.state.fields_written);
                }
                Some(expected) if expected != self.state.fields_written => {
                    return Err(Error::new(ErrorKind::UnequalLengths {
                        pos: None,
                        expected_len: expected,
                        len: self.state.fields_written,
                    }))
                }
                Some(_) => {}
            }
        }
        Ok(())
    }
}

impl Buffer {
    /// Returns a slice of the buffer's current contents.
    ///
    /// The slice returned may be empty.
    #[inline]
    fn readable(&self) -> &[u8] {
        &self.buf[..self.len]
    }

    /// Returns a mutable slice of the remaining space in this buffer.
    ///
    /// The slice returned may be empty.
    #[inline]
    fn writable(&mut self) -> &mut [u8] {
        &mut self.buf[self.len..]
    }

    /// Indicates that `n` bytes have been written to this buffer.
    #[inline]
    fn written(&mut self, n: usize) {
        self.len += n;
    }

    /// Clear the buffer.
    #[inline]
    fn clear(&mut self) {
        self.len = 0;
    }
}

#[cfg(test)]
mod tests {
    use serde::{serde_if_integer128, Serialize};

    use std::io::{self, Write};

    use crate::byte_record::ByteRecord;
    use crate::error::ErrorKind;
    use crate::string_record::StringRecord;

    use super::{Writer, WriterBuilder};

    fn wtr_as_string(wtr: Writer<Vec<u8>>) -> String {
        String::from_utf8(wtr.into_inner().unwrap()).unwrap()
    }

    #[test]
    fn one_record() {
        let mut wtr = WriterBuilder::new().from_writer(vec![]);
        wtr.write_record(&["a", "b", "c"]).unwrap();

        assert_eq!(wtr_as_string(wtr), "a,b,c\n");
    }

    #[test]
    fn one_string_record() {
        let mut wtr = WriterBuilder::new().from_writer(vec![]);
        wtr.write_record(&StringRecord::from(vec!["a", "b", "c"])).unwrap();

        assert_eq!(wtr_as_string(wtr), "a,b,c\n");
    }

    #[test]
    fn one_byte_record() {
        let mut wtr = WriterBuilder::new().from_writer(vec![]);
        wtr.write_record(&ByteRecord::from(vec!["a", "b", "c"])).unwrap();

        assert_eq!(wtr_as_string(wtr), "a,b,c\n");
    }

    #[test]
    fn raw_one_byte_record() {
        let mut wtr = WriterBuilder::new().from_writer(vec![]);
        wtr.write_byte_record(&ByteRecord::from(vec!["a", "b", "c"])).unwrap();

        assert_eq!(wtr_as_string(wtr), "a,b,c\n");
    }

    #[test]
    fn one_empty_record() {
        let mut wtr = WriterBuilder::new().from_writer(vec![]);
        wtr.write_record(&[""]).unwrap();

        assert_eq!(wtr_as_string(wtr), "\"\"\n");
    }

    #[test]
    fn raw_one_empty_record() {
        let mut wtr = WriterBuilder::new().from_writer(vec![]);
        wtr.write_byte_record(&ByteRecord::from(vec![""])).unwrap();

        assert_eq!(wtr_as_string(wtr), "\"\"\n");
    }

    #[test]
    fn two_empty_records() {
        let mut wtr = WriterBuilder::new().from_writer(vec![]);
        wtr.write_record(&[""]).unwrap();
        wtr.write_record(&[""]).unwrap();

        assert_eq!(wtr_as_string(wtr), "\"\"\n\"\"\n");
    }

    #[test]
    fn raw_two_empty_records() {
        let mut wtr = WriterBuilder::new().from_writer(vec![]);
        wtr.write_byte_record(&ByteRecord::from(vec![""])).unwrap();
        wtr.write_byte_record(&ByteRecord::from(vec![""])).unwrap();

        assert_eq!(wtr_as_string(wtr), "\"\"\n\"\"\n");
    }

    #[test]
    fn unequal_records_bad() {
        let mut wtr = WriterBuilder::new().from_writer(vec![]);
        wtr.write_record(&ByteRecord::from(vec!["a", "b", "c"])).unwrap();
        let err = wtr.write_record(&ByteRecord::from(vec!["a"])).unwrap_err();
        match *err.kind() {
            ErrorKind::UnequalLengths { ref pos, expected_len, len } => {
                assert!(pos.is_none());
                assert_eq!(expected_len, 3);
                assert_eq!(len, 1);
            }
            ref x => {
                panic!("expected UnequalLengths error, but got '{:?}'", x);
            }
        }
    }

    #[test]
    fn raw_unequal_records_bad() {
        let mut wtr = WriterBuilder::new().from_writer(vec![]);
        wtr.write_byte_record(&ByteRecord::from(vec!["a", "b", "c"])).unwrap();
        let err =
            wtr.write_byte_record(&ByteRecord::from(vec!["a"])).unwrap_err();
        match *err.kind() {
            ErrorKind::UnequalLengths { ref pos, expected_len, len } => {
                assert!(pos.is_none());
                assert_eq!(expected_len, 3);
                assert_eq!(len, 1);
            }
            ref x => {
                panic!("expected UnequalLengths error, but got '{:?}'", x);
            }
        }
    }

    #[test]
    fn unequal_records_ok() {
        let mut wtr = WriterBuilder::new().flexible(true).from_writer(vec![]);
        wtr.write_record(&ByteRecord::from(vec!["a", "b", "c"])).unwrap();
        wtr.write_record(&ByteRecord::from(vec!["a"])).unwrap();
        assert_eq!(wtr_as_string(wtr), "a,b,c\na\n");
    }

    #[test]
    fn raw_unequal_records_ok() {
        let mut wtr = WriterBuilder::new().flexible(true).from_writer(vec![]);
        wtr.write_byte_record(&ByteRecord::from(vec!["a", "b", "c"])).unwrap();
        wtr.write_byte_record(&ByteRecord::from(vec!["a"])).unwrap();
        assert_eq!(wtr_as_string(wtr), "a,b,c\na\n");
    }

    #[test]
    fn full_buffer_should_not_flush_underlying() {
        struct MarkWriteAndFlush(Vec<u8>);

        impl MarkWriteAndFlush {
            fn to_str(self) -> String {
                String::from_utf8(self.0).unwrap()
            }
        }

        impl Write for MarkWriteAndFlush {
            fn write(&mut self, data: &[u8]) -> io::Result<usize> {
                self.0.write(b">")?;
                let written = self.0.write(data)?;
                self.0.write(b"<")?;

                Ok(written)
            }

            fn flush(&mut self) -> io::Result<()> {
                self.0.write(b"!")?;
                Ok(())
            }
        }

        let underlying = MarkWriteAndFlush(vec![]);
        let mut wtr =
            WriterBuilder::new().buffer_capacity(4).from_writer(underlying);

        wtr.write_byte_record(&ByteRecord::from(vec!["a", "b"])).unwrap();
        wtr.write_byte_record(&ByteRecord::from(vec!["c", "d"])).unwrap();
        wtr.flush().unwrap();
        wtr.write_byte_record(&ByteRecord::from(vec!["e", "f"])).unwrap();

        let got = wtr.into_inner().unwrap().to_str();

        // As the buffer size is 4 we should write each record separately, and
        // flush when explicitly called and implictly in into_inner.
        assert_eq!(got, ">a,b\n<>c,d\n<!>e,f\n<!");
    }

    #[test]
    fn serialize_with_headers() {
        #[derive(Serialize)]
        struct Row {
            foo: i32,
            bar: f64,
            baz: bool,
        }

        let mut wtr = WriterBuilder::new().from_writer(vec![]);
        wtr.serialize(Row { foo: 42, bar: 42.5, baz: true }).unwrap();
        assert_eq!(wtr_as_string(wtr), "foo,bar,baz\n42,42.5,true\n");
    }

    #[test]
    fn serialize_no_headers() {
        #[derive(Serialize)]
        struct Row {
            foo: i32,
            bar: f64,
            baz: bool,
        }

        let mut wtr =
            WriterBuilder::new().has_headers(false).from_writer(vec![]);
        wtr.serialize(Row { foo: 42, bar: 42.5, baz: true }).unwrap();
        assert_eq!(wtr_as_string(wtr), "42,42.5,true\n");
    }

    serde_if_integer128! {
        #[test]
        fn serialize_no_headers_128() {
            #[derive(Serialize)]
            struct Row {
                foo: i128,
                bar: f64,
                baz: bool,
            }

            let mut wtr =
                WriterBuilder::new().has_headers(false).from_writer(vec![]);
            wtr.serialize(Row {
                foo: 9_223_372_036_854_775_808,
                bar: 42.5,
                baz: true,
            }).unwrap();
            assert_eq!(wtr_as_string(wtr), "9223372036854775808,42.5,true\n");
        }
    }

    #[test]
    fn serialize_tuple() {
        let mut wtr = WriterBuilder::new().from_writer(vec![]);
        wtr.serialize((true, 1.3, "hi")).unwrap();
        assert_eq!(wtr_as_string(wtr), "true,1.3,hi\n");
    }
}
