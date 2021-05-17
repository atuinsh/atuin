use std::fs::File;
use std::io::{self, BufRead, Seek};
use std::marker::PhantomData;
use std::path::Path;
use std::result;

use csv_core::{Reader as CoreReader, ReaderBuilder as CoreReaderBuilder};
use serde::de::DeserializeOwned;

use crate::byte_record::{ByteRecord, Position};
use crate::error::{Error, ErrorKind, Result, Utf8Error};
use crate::string_record::StringRecord;
use crate::{Terminator, Trim};

/// Builds a CSV reader with various configuration knobs.
///
/// This builder can be used to tweak the field delimiter, record terminator
/// and more. Once a CSV `Reader` is built, its configuration cannot be
/// changed.
#[derive(Debug)]
pub struct ReaderBuilder {
    capacity: usize,
    flexible: bool,
    has_headers: bool,
    trim: Trim,
    /// The underlying CSV parser builder.
    ///
    /// We explicitly put this on the heap because CoreReaderBuilder embeds an
    /// entire DFA transition table, which along with other things, tallies up
    /// to almost 500 bytes on the stack.
    builder: Box<CoreReaderBuilder>,
}

impl Default for ReaderBuilder {
    fn default() -> ReaderBuilder {
        ReaderBuilder {
            capacity: 8 * (1 << 10),
            flexible: false,
            has_headers: true,
            trim: Trim::default(),
            builder: Box::new(CoreReaderBuilder::default()),
        }
    }
}

impl ReaderBuilder {
    /// Create a new builder for configuring CSV parsing.
    ///
    /// To convert a builder into a reader, call one of the methods starting
    /// with `from_`.
    ///
    /// # Example
    ///
    /// ```
    /// use std::error::Error;
    /// use csv::{ReaderBuilder, StringRecord};
    ///
    /// # fn main() { example().unwrap(); }
    /// fn example() -> Result<(), Box<dyn Error>> {
    ///     let data = "\
    /// city,country,pop
    /// Boston,United States,4628910
    /// Concord,United States,42695
    /// ";
    ///     let mut rdr = ReaderBuilder::new().from_reader(data.as_bytes());
    ///
    ///     let records = rdr
    ///         .records()
    ///         .collect::<Result<Vec<StringRecord>, csv::Error>>()?;
    ///     assert_eq!(records, vec![
    ///         vec!["Boston", "United States", "4628910"],
    ///         vec!["Concord", "United States", "42695"],
    ///     ]);
    ///     Ok(())
    /// }
    /// ```
    pub fn new() -> ReaderBuilder {
        ReaderBuilder::default()
    }

    /// Build a CSV parser from this configuration that reads data from the
    /// given file path.
    ///
    /// If there was a problem opening the file at the given path, then this
    /// returns the corresponding error.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use std::error::Error;
    /// use csv::ReaderBuilder;
    ///
    /// # fn main() { example().unwrap(); }
    /// fn example() -> Result<(), Box<dyn Error>> {
    ///     let mut rdr = ReaderBuilder::new().from_path("foo.csv")?;
    ///     for result in rdr.records() {
    ///         let record = result?;
    ///         println!("{:?}", record);
    ///     }
    ///     Ok(())
    /// }
    /// ```
    pub fn from_path<P: AsRef<Path>>(&self, path: P) -> Result<Reader<File>> {
        Ok(Reader::new(self, File::open(path)?))
    }

    /// Build a CSV parser from this configuration that reads data from `rdr`.
    ///
    /// Note that the CSV reader is buffered automatically, so you should not
    /// wrap `rdr` in a buffered reader like `io::BufReader`.
    ///
    /// # Example
    ///
    /// ```
    /// use std::error::Error;
    /// use csv::ReaderBuilder;
    ///
    /// # fn main() { example().unwrap(); }
    /// fn example() -> Result<(), Box<dyn Error>> {
    ///     let data = "\
    /// city,country,pop
    /// Boston,United States,4628910
    /// Concord,United States,42695
    /// ";
    ///     let mut rdr = ReaderBuilder::new().from_reader(data.as_bytes());
    ///     for result in rdr.records() {
    ///         let record = result?;
    ///         println!("{:?}", record);
    ///     }
    ///     Ok(())
    /// }
    /// ```
    pub fn from_reader<R: io::Read>(&self, rdr: R) -> Reader<R> {
        Reader::new(self, rdr)
    }

    /// The field delimiter to use when parsing CSV.
    ///
    /// The default is `b','`.
    ///
    /// # Example
    ///
    /// ```
    /// use std::error::Error;
    /// use csv::ReaderBuilder;
    ///
    /// # fn main() { example().unwrap(); }
    /// fn example() -> Result<(), Box<dyn Error>> {
    ///     let data = "\
    /// city;country;pop
    /// Boston;United States;4628910
    /// ";
    ///     let mut rdr = ReaderBuilder::new()
    ///         .delimiter(b';')
    ///         .from_reader(data.as_bytes());
    ///
    ///     if let Some(result) = rdr.records().next() {
    ///         let record = result?;
    ///         assert_eq!(record, vec!["Boston", "United States", "4628910"]);
    ///         Ok(())
    ///     } else {
    ///         Err(From::from("expected at least one record but got none"))
    ///     }
    /// }
    /// ```
    pub fn delimiter(&mut self, delimiter: u8) -> &mut ReaderBuilder {
        self.builder.delimiter(delimiter);
        self
    }

    /// Whether to treat the first row as a special header row.
    ///
    /// By default, the first row is treated as a special header row, which
    /// means the header is never returned by any of the record reading methods
    /// or iterators. When this is disabled (`yes` set to `false`), the first
    /// row is not treated specially.
    ///
    /// Note that the `headers` and `byte_headers` methods are unaffected by
    /// whether this is set. Those methods always return the first record.
    ///
    /// # Example
    ///
    /// This example shows what happens when `has_headers` is disabled.
    /// Namely, the first row is treated just like any other row.
    ///
    /// ```
    /// use std::error::Error;
    /// use csv::ReaderBuilder;
    ///
    /// # fn main() { example().unwrap(); }
    /// fn example() -> Result<(), Box<dyn Error>> {
    ///     let data = "\
    /// city,country,pop
    /// Boston,United States,4628910
    /// ";
    ///     let mut rdr = ReaderBuilder::new()
    ///         .has_headers(false)
    ///         .from_reader(data.as_bytes());
    ///     let mut iter = rdr.records();
    ///
    ///     // Read the first record.
    ///     if let Some(result) = iter.next() {
    ///         let record = result?;
    ///         assert_eq!(record, vec!["city", "country", "pop"]);
    ///     } else {
    ///         return Err(From::from(
    ///             "expected at least two records but got none"));
    ///     }
    ///
    ///     // Read the second record.
    ///     if let Some(result) = iter.next() {
    ///         let record = result?;
    ///         assert_eq!(record, vec!["Boston", "United States", "4628910"]);
    ///     } else {
    ///         return Err(From::from(
    ///             "expected at least two records but got one"))
    ///     }
    ///     Ok(())
    /// }
    /// ```
    pub fn has_headers(&mut self, yes: bool) -> &mut ReaderBuilder {
        self.has_headers = yes;
        self
    }

    /// Whether the number of fields in records is allowed to change or not.
    ///
    /// When disabled (which is the default), parsing CSV data will return an
    /// error if a record is found with a number of fields different from the
    /// number of fields in a previous record.
    ///
    /// When enabled, this error checking is turned off.
    ///
    /// # Example: flexible records enabled
    ///
    /// ```
    /// use std::error::Error;
    /// use csv::ReaderBuilder;
    ///
    /// # fn main() { example().unwrap(); }
    /// fn example() -> Result<(), Box<dyn Error>> {
    ///     // Notice that the first row is missing the population count.
    ///     let data = "\
    /// city,country,pop
    /// Boston,United States
    /// ";
    ///     let mut rdr = ReaderBuilder::new()
    ///         .flexible(true)
    ///         .from_reader(data.as_bytes());
    ///
    ///     if let Some(result) = rdr.records().next() {
    ///         let record = result?;
    ///         assert_eq!(record, vec!["Boston", "United States"]);
    ///         Ok(())
    ///     } else {
    ///         Err(From::from("expected at least one record but got none"))
    ///     }
    /// }
    /// ```
    ///
    /// # Example: flexible records disabled
    ///
    /// This shows the error that appears when records of unequal length
    /// are found and flexible records have been disabled (which is the
    /// default).
    ///
    /// ```
    /// use std::error::Error;
    /// use csv::{ErrorKind, ReaderBuilder};
    ///
    /// # fn main() { example().unwrap(); }
    /// fn example() -> Result<(), Box<dyn Error>> {
    ///     // Notice that the first row is missing the population count.
    ///     let data = "\
    /// city,country,pop
    /// Boston,United States
    /// ";
    ///     let mut rdr = ReaderBuilder::new()
    ///         .flexible(false)
    ///         .from_reader(data.as_bytes());
    ///
    ///     if let Some(Err(err)) = rdr.records().next() {
    ///         match *err.kind() {
    ///             ErrorKind::UnequalLengths { expected_len, len, .. } => {
    ///                 // The header row has 3 fields...
    ///                 assert_eq!(expected_len, 3);
    ///                 // ... but the first row has only 2 fields.
    ///                 assert_eq!(len, 2);
    ///                 Ok(())
    ///             }
    ///             ref wrong => {
    ///                 Err(From::from(format!(
    ///                     "expected UnequalLengths error but got {:?}",
    ///                     wrong)))
    ///             }
    ///         }
    ///     } else {
    ///         Err(From::from(
    ///             "expected at least one errored record but got none"))
    ///     }
    /// }
    /// ```
    pub fn flexible(&mut self, yes: bool) -> &mut ReaderBuilder {
        self.flexible = yes;
        self
    }

    /// Whether fields are trimmed of leading and trailing whitespace or not.
    ///
    /// By default, no trimming is performed. This method permits one to
    /// override that behavior and choose one of the following options:
    ///
    /// 1. `Trim::Headers` trims only header values.
    /// 2. `Trim::Fields` trims only non-header or "field" values.
    /// 3. `Trim::All` trims both header and non-header values.
    ///
    /// A value is only interpreted as a header value if this CSV reader is
    /// configured to read a header record (which is the default).
    ///
    /// When reading string records, characters meeting the definition of
    /// Unicode whitespace are trimmed. When reading byte records, characters
    /// meeting the definition of ASCII whitespace are trimmed. ASCII
    /// whitespace characters correspond to the set `[\t\n\v\f\r ]`.
    ///
    /// # Example
    ///
    /// This example shows what happens when all values are trimmed.
    ///
    /// ```
    /// use std::error::Error;
    /// use csv::{ReaderBuilder, StringRecord, Trim};
    ///
    /// # fn main() { example().unwrap(); }
    /// fn example() -> Result<(), Box<dyn Error>> {
    ///     let data = "\
    /// city ,   country ,  pop
    /// Boston,\"
    ///    United States\",4628910
    /// Concord,   United States   ,42695
    /// ";
    ///     let mut rdr = ReaderBuilder::new()
    ///         .trim(Trim::All)
    ///         .from_reader(data.as_bytes());
    ///     let records = rdr
    ///         .records()
    ///         .collect::<Result<Vec<StringRecord>, csv::Error>>()?;
    ///     assert_eq!(records, vec![
    ///         vec!["Boston", "United States", "4628910"],
    ///         vec!["Concord", "United States", "42695"],
    ///     ]);
    ///     Ok(())
    /// }
    /// ```
    pub fn trim(&mut self, trim: Trim) -> &mut ReaderBuilder {
        self.trim = trim;
        self
    }

    /// The record terminator to use when parsing CSV.
    ///
    /// A record terminator can be any single byte. The default is a special
    /// value, `Terminator::CRLF`, which treats any occurrence of `\r`, `\n`
    /// or `\r\n` as a single record terminator.
    ///
    /// # Example: `$` as a record terminator
    ///
    /// ```
    /// use std::error::Error;
    /// use csv::{ReaderBuilder, Terminator};
    ///
    /// # fn main() { example().unwrap(); }
    /// fn example() -> Result<(), Box<dyn Error>> {
    ///     let data = "city,country,pop$Boston,United States,4628910";
    ///     let mut rdr = ReaderBuilder::new()
    ///         .terminator(Terminator::Any(b'$'))
    ///         .from_reader(data.as_bytes());
    ///
    ///     if let Some(result) = rdr.records().next() {
    ///         let record = result?;
    ///         assert_eq!(record, vec!["Boston", "United States", "4628910"]);
    ///         Ok(())
    ///     } else {
    ///         Err(From::from("expected at least one record but got none"))
    ///     }
    /// }
    /// ```
    pub fn terminator(&mut self, term: Terminator) -> &mut ReaderBuilder {
        self.builder.terminator(term.to_core());
        self
    }

    /// The quote character to use when parsing CSV.
    ///
    /// The default is `b'"'`.
    ///
    /// # Example: single quotes instead of double quotes
    ///
    /// ```
    /// use std::error::Error;
    /// use csv::ReaderBuilder;
    ///
    /// # fn main() { example().unwrap(); }
    /// fn example() -> Result<(), Box<dyn Error>> {
    ///     let data = "\
    /// city,country,pop
    /// Boston,'United States',4628910
    /// ";
    ///     let mut rdr = ReaderBuilder::new()
    ///         .quote(b'\'')
    ///         .from_reader(data.as_bytes());
    ///
    ///     if let Some(result) = rdr.records().next() {
    ///         let record = result?;
    ///         assert_eq!(record, vec!["Boston", "United States", "4628910"]);
    ///         Ok(())
    ///     } else {
    ///         Err(From::from("expected at least one record but got none"))
    ///     }
    /// }
    /// ```
    pub fn quote(&mut self, quote: u8) -> &mut ReaderBuilder {
        self.builder.quote(quote);
        self
    }

    /// The escape character to use when parsing CSV.
    ///
    /// In some variants of CSV, quotes are escaped using a special escape
    /// character like `\` (instead of escaping quotes by doubling them).
    ///
    /// By default, recognizing these idiosyncratic escapes is disabled.
    ///
    /// # Example
    ///
    /// ```
    /// use std::error::Error;
    /// use csv::ReaderBuilder;
    ///
    /// # fn main() { example().unwrap(); }
    /// fn example() -> Result<(), Box<dyn Error>> {
    ///     let data = "\
    /// city,country,pop
    /// Boston,\"The \\\"United\\\" States\",4628910
    /// ";
    ///     let mut rdr = ReaderBuilder::new()
    ///         .escape(Some(b'\\'))
    ///         .from_reader(data.as_bytes());
    ///
    ///     if let Some(result) = rdr.records().next() {
    ///         let record = result?;
    ///         assert_eq!(record, vec![
    ///             "Boston", "The \"United\" States", "4628910",
    ///         ]);
    ///         Ok(())
    ///     } else {
    ///         Err(From::from("expected at least one record but got none"))
    ///     }
    /// }
    /// ```
    pub fn escape(&mut self, escape: Option<u8>) -> &mut ReaderBuilder {
        self.builder.escape(escape);
        self
    }

    /// Enable double quote escapes.
    ///
    /// This is enabled by default, but it may be disabled. When disabled,
    /// doubled quotes are not interpreted as escapes.
    ///
    /// # Example
    ///
    /// ```
    /// use std::error::Error;
    /// use csv::ReaderBuilder;
    ///
    /// # fn main() { example().unwrap(); }
    /// fn example() -> Result<(), Box<dyn Error>> {
    ///     let data = "\
    /// city,country,pop
    /// Boston,\"The \"\"United\"\" States\",4628910
    /// ";
    ///     let mut rdr = ReaderBuilder::new()
    ///         .double_quote(false)
    ///         .from_reader(data.as_bytes());
    ///
    ///     if let Some(result) = rdr.records().next() {
    ///         let record = result?;
    ///         assert_eq!(record, vec![
    ///             "Boston", "The \"United\"\" States\"", "4628910",
    ///         ]);
    ///         Ok(())
    ///     } else {
    ///         Err(From::from("expected at least one record but got none"))
    ///     }
    /// }
    /// ```
    pub fn double_quote(&mut self, yes: bool) -> &mut ReaderBuilder {
        self.builder.double_quote(yes);
        self
    }

    /// Enable or disable quoting.
    ///
    /// This is enabled by default, but it may be disabled. When disabled,
    /// quotes are not treated specially.
    ///
    /// # Example
    ///
    /// ```
    /// use std::error::Error;
    /// use csv::ReaderBuilder;
    ///
    /// # fn main() { example().unwrap(); }
    /// fn example() -> Result<(), Box<dyn Error>> {
    ///     let data = "\
    /// city,country,pop
    /// Boston,\"The United States,4628910
    /// ";
    ///     let mut rdr = ReaderBuilder::new()
    ///         .quoting(false)
    ///         .from_reader(data.as_bytes());
    ///
    ///     if let Some(result) = rdr.records().next() {
    ///         let record = result?;
    ///         assert_eq!(record, vec![
    ///             "Boston", "\"The United States", "4628910",
    ///         ]);
    ///         Ok(())
    ///     } else {
    ///         Err(From::from("expected at least one record but got none"))
    ///     }
    /// }
    /// ```
    pub fn quoting(&mut self, yes: bool) -> &mut ReaderBuilder {
        self.builder.quoting(yes);
        self
    }

    /// The comment character to use when parsing CSV.
    ///
    /// If the start of a record begins with the byte given here, then that
    /// line is ignored by the CSV parser.
    ///
    /// This is disabled by default.
    ///
    /// # Example
    ///
    /// ```
    /// use std::error::Error;
    /// use csv::ReaderBuilder;
    ///
    /// # fn main() { example().unwrap(); }
    /// fn example() -> Result<(), Box<dyn Error>> {
    ///     let data = "\
    /// city,country,pop
    /// #Concord,United States,42695
    /// Boston,United States,4628910
    /// ";
    ///     let mut rdr = ReaderBuilder::new()
    ///         .comment(Some(b'#'))
    ///         .from_reader(data.as_bytes());
    ///
    ///     if let Some(result) = rdr.records().next() {
    ///         let record = result?;
    ///         assert_eq!(record, vec!["Boston", "United States", "4628910"]);
    ///         Ok(())
    ///     } else {
    ///         Err(From::from("expected at least one record but got none"))
    ///     }
    /// }
    /// ```
    pub fn comment(&mut self, comment: Option<u8>) -> &mut ReaderBuilder {
        self.builder.comment(comment);
        self
    }

    /// A convenience method for specifying a configuration to read ASCII
    /// delimited text.
    ///
    /// This sets the delimiter and record terminator to the ASCII unit
    /// separator (`\x1F`) and record separator (`\x1E`), respectively.
    ///
    /// # Example
    ///
    /// ```
    /// use std::error::Error;
    /// use csv::ReaderBuilder;
    ///
    /// # fn main() { example().unwrap(); }
    /// fn example() -> Result<(), Box<dyn Error>> {
    ///     let data = "\
    /// city\x1Fcountry\x1Fpop\x1EBoston\x1FUnited States\x1F4628910";
    ///     let mut rdr = ReaderBuilder::new()
    ///         .ascii()
    ///         .from_reader(data.as_bytes());
    ///
    ///     if let Some(result) = rdr.records().next() {
    ///         let record = result?;
    ///         assert_eq!(record, vec!["Boston", "United States", "4628910"]);
    ///         Ok(())
    ///     } else {
    ///         Err(From::from("expected at least one record but got none"))
    ///     }
    /// }
    /// ```
    pub fn ascii(&mut self) -> &mut ReaderBuilder {
        self.builder.ascii();
        self
    }

    /// Set the capacity (in bytes) of the buffer used in the CSV reader.
    /// This defaults to a reasonable setting.
    pub fn buffer_capacity(&mut self, capacity: usize) -> &mut ReaderBuilder {
        self.capacity = capacity;
        self
    }

    /// Enable or disable the NFA for parsing CSV.
    ///
    /// This is intended to be a debug option. The NFA is always slower than
    /// the DFA.
    #[doc(hidden)]
    pub fn nfa(&mut self, yes: bool) -> &mut ReaderBuilder {
        self.builder.nfa(yes);
        self
    }
}

/// A already configured CSV reader.
///
/// A CSV reader takes as input CSV data and transforms that into standard Rust
/// values. The most flexible way to read CSV data is as a sequence of records,
/// where a record is a sequence of fields and each field is a string. However,
/// a reader can also deserialize CSV data into Rust types like `i64` or
/// `(String, f64, f64, f64)` or even a custom struct automatically using
/// Serde.
///
/// # Configuration
///
/// A CSV reader has a couple convenient constructor methods like `from_path`
/// and `from_reader`. However, if you want to configure the CSV reader to use
/// a different delimiter or quote character (among many other things), then
/// you should use a [`ReaderBuilder`](struct.ReaderBuilder.html) to construct
/// a `Reader`. For example, to change the field delimiter:
///
/// ```
/// use std::error::Error;
/// use csv::ReaderBuilder;
///
/// # fn main() { example().unwrap(); }
/// fn example() -> Result<(), Box<dyn Error>> {
///     let data = "\
/// city;country;pop
/// Boston;United States;4628910
/// ";
///     let mut rdr = ReaderBuilder::new()
///         .delimiter(b';')
///         .from_reader(data.as_bytes());
///
///     if let Some(result) = rdr.records().next() {
///         let record = result?;
///         assert_eq!(record, vec!["Boston", "United States", "4628910"]);
///         Ok(())
///     } else {
///         Err(From::from("expected at least one record but got none"))
///     }
/// }
/// ```
///
/// # Error handling
///
/// In general, CSV *parsing* does not ever return an error. That is, there is
/// no such thing as malformed CSV data. Instead, this reader will prioritize
/// finding a parse over rejecting CSV data that it does not understand. This
/// choice was inspired by other popular CSV parsers, but also because it is
/// pragmatic. CSV data varies wildly, so even if the CSV data is malformed,
/// it might still be possible to work with the data. In the land of CSV, there
/// is no "right" or "wrong," only "right" and "less right."
///
/// With that said, a number of errors can occur while reading CSV data:
///
/// * By default, all records in CSV data must have the same number of fields.
///   If a record is found with a different number of fields than a prior
///   record, then an error is returned. This behavior can be disabled by
///   enabling flexible parsing via the `flexible` method on
///   [`ReaderBuilder`](struct.ReaderBuilder.html).
/// * When reading CSV data from a resource (like a file), it is possible for
///   reading from the underlying resource to fail. This will return an error.
///   For subsequent calls to the `Reader` after encountering a such error
///   (unless `seek` is used), it will behave as if end of file had been
///   reached, in order to avoid running into infinite loops when still
///   attempting to read the next record when one has errored.
/// * When reading CSV data into `String` or `&str` fields (e.g., via a
///   [`StringRecord`](struct.StringRecord.html)), UTF-8 is strictly
///   enforced. If CSV data is invalid UTF-8, then an error is returned. If
///   you want to read invalid UTF-8, then you should use the byte oriented
///   APIs such as [`ByteRecord`](struct.ByteRecord.html). If you need explicit
///   support for another encoding entirely, then you'll need to use another
///   crate to transcode your CSV data to UTF-8 before parsing it.
/// * When using Serde to deserialize CSV data into Rust types, it is possible
///   for a number of additional errors to occur. For example, deserializing
///   a field `xyz` into an `i32` field will result in an error.
///
/// For more details on the precise semantics of errors, see the
/// [`Error`](enum.Error.html) type.
#[derive(Debug)]
pub struct Reader<R> {
    /// The underlying CSV parser.
    ///
    /// We explicitly put this on the heap because CoreReader embeds an entire
    /// DFA transition table, which along with other things, tallies up to
    /// almost 500 bytes on the stack.
    core: Box<CoreReader>,
    /// The underlying reader.
    rdr: io::BufReader<R>,
    /// Various state tracking.
    ///
    /// There is more state embedded in the `CoreReader`.
    state: ReaderState,
}

#[derive(Debug)]
struct ReaderState {
    /// When set, this contains the first row of any parsed CSV data.
    ///
    /// This is always populated, regardless of whether `has_headers` is set.
    headers: Option<Headers>,
    /// When set, the first row of parsed CSV data is excluded from things
    /// that read records, like iterators and `read_record`.
    has_headers: bool,
    /// When set, there is no restriction on the length of records. When not
    /// set, every record must have the same number of fields, or else an error
    /// is reported.
    flexible: bool,
    trim: Trim,
    /// The number of fields in the first record parsed.
    first_field_count: Option<u64>,
    /// The current position of the parser.
    ///
    /// Note that this position is only observable by callers at the start
    /// of a record. More granular positions are not supported.
    cur_pos: Position,
    /// Whether the first record has been read or not.
    first: bool,
    /// Whether the reader has been seeked or not.
    seeked: bool,
    /// Whether EOF of the underlying reader has been reached or not.
    ///
    /// IO errors on the underlying reader will be considered as an EOF for
    /// subsequent read attempts, as it would be incorrect to keep on trying
    /// to read when the underlying reader has broken.
    ///
    /// For clarity, having the best `Debug` impl and in case they need to be
    /// treated differently at some point, we store whether the `EOF` is
    /// considered because an actual EOF happened, or because we encoundered
    /// an IO error.
    /// This has no additional runtime cost.
    eof: ReaderEofState,
}

/// Whether EOF of the underlying reader has been reached or not.
///
/// IO errors on the underlying reader will be considered as an EOF for
/// subsequent read attempts, as it would be incorrect to keep on trying
/// to read when the underlying reader has broken.
///
/// For clarity, having the best `Debug` impl and in case they need to be
/// treated differently at some point, we store whether the `EOF` is
/// considered because an actual EOF happened, or because we encoundered
/// an IO error
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReaderEofState {
    NotEof,
    Eof,
    IOError,
}

/// Headers encapsulates any data associated with the headers of CSV data.
///
/// The headers always correspond to the first row.
#[derive(Debug)]
struct Headers {
    /// The header, as raw bytes.
    byte_record: ByteRecord,
    /// The header, as valid UTF-8 (or a UTF-8 error).
    string_record: result::Result<StringRecord, Utf8Error>,
}

impl Reader<Reader<File>> {
    /// Create a new CSV parser with a default configuration for the given
    /// file path.
    ///
    /// To customize CSV parsing, use a `ReaderBuilder`.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use std::error::Error;
    /// use csv::Reader;
    ///
    /// # fn main() { example().unwrap(); }
    /// fn example() -> Result<(), Box<dyn Error>> {
    ///     let mut rdr = Reader::from_path("foo.csv")?;
    ///     for result in rdr.records() {
    ///         let record = result?;
    ///         println!("{:?}", record);
    ///     }
    ///     Ok(())
    /// }
    /// ```
    pub fn from_path<P: AsRef<Path>>(path: P) -> Result<Reader<File>> {
        ReaderBuilder::new().from_path(path)
    }
}

impl<R: io::Read> Reader<R> {
    /// Create a new CSV reader given a builder and a source of underlying
    /// bytes.
    fn new(builder: &ReaderBuilder, rdr: R) -> Reader<R> {
        Reader {
            core: Box::new(builder.builder.build()),
            rdr: io::BufReader::with_capacity(builder.capacity, rdr),
            state: ReaderState {
                headers: None,
                has_headers: builder.has_headers,
                flexible: builder.flexible,
                trim: builder.trim,
                first_field_count: None,
                cur_pos: Position::new(),
                first: false,
                seeked: false,
                eof: ReaderEofState::NotEof,
            },
        }
    }

    /// Create a new CSV parser with a default configuration for the given
    /// reader.
    ///
    /// To customize CSV parsing, use a `ReaderBuilder`.
    ///
    /// # Example
    ///
    /// ```
    /// use std::error::Error;
    /// use csv::Reader;
    ///
    /// # fn main() { example().unwrap(); }
    /// fn example() -> Result<(), Box<dyn Error>> {
    ///     let data = "\
    /// city,country,pop
    /// Boston,United States,4628910
    /// Concord,United States,42695
    /// ";
    ///     let mut rdr = Reader::from_reader(data.as_bytes());
    ///     for result in rdr.records() {
    ///         let record = result?;
    ///         println!("{:?}", record);
    ///     }
    ///     Ok(())
    /// }
    /// ```
    pub fn from_reader(rdr: R) -> Reader<R> {
        ReaderBuilder::new().from_reader(rdr)
    }

    /// Returns a borrowed iterator over deserialized records.
    ///
    /// Each item yielded by this iterator is a `Result<D, Error>`.
    /// Therefore, in order to access the record, callers must handle the
    /// possibility of error (typically with `try!` or `?`).
    ///
    /// If `has_headers` was enabled via a `ReaderBuilder` (which is the
    /// default), then this does not include the first record. Additionally,
    /// if `has_headers` is enabled, then deserializing into a struct will
    /// automatically align the values in each row to the fields of a struct
    /// based on the header row.
    ///
    /// # Example
    ///
    /// This shows how to deserialize CSV data into normal Rust structs. The
    /// fields of the header row are used to match up the values in each row
    /// to the fields of the struct.
    ///
    /// ```
    /// use std::error::Error;
    ///
    /// use csv::Reader;
    /// use serde::Deserialize;
    ///
    /// #[derive(Debug, Deserialize, Eq, PartialEq)]
    /// struct Row {
    ///     city: String,
    ///     country: String,
    ///     #[serde(rename = "popcount")]
    ///     population: u64,
    /// }
    ///
    /// # fn main() { example().unwrap(); }
    /// fn example() -> Result<(), Box<dyn Error>> {
    ///     let data = "\
    /// city,country,popcount
    /// Boston,United States,4628910
    /// ";
    ///     let mut rdr = Reader::from_reader(data.as_bytes());
    ///     let mut iter = rdr.deserialize();
    ///
    ///     if let Some(result) = iter.next() {
    ///         let record: Row = result?;
    ///         assert_eq!(record, Row {
    ///             city: "Boston".to_string(),
    ///             country: "United States".to_string(),
    ///             population: 4628910,
    ///         });
    ///         Ok(())
    ///     } else {
    ///         Err(From::from("expected at least one record but got none"))
    ///     }
    /// }
    /// ```
    ///
    /// # Rules
    ///
    /// For the most part, any Rust type that maps straight-forwardly to a CSV
    /// record is supported. This includes maps, structs, tuples and tuple
    /// structs. Other Rust types, such as `Vec`s, arrays, and enums have
    /// a more complicated story. In general, when working with CSV data, one
    /// should avoid *nested sequences* as much as possible.
    ///
    /// Maps, structs, tuples and tuple structs map to CSV records in a simple
    /// way. Tuples and tuple structs decode their fields in the order that
    /// they are defined. Structs will do the same only if `has_headers` has
    /// been disabled using [`ReaderBuilder`](struct.ReaderBuilder.html),
    /// otherwise, structs and maps are deserialized based on the fields
    /// defined in the header row. (If there is no header row, then
    /// deserializing into a map will result in an error.)
    ///
    /// Nested sequences are supported in a limited capacity. Namely, they
    /// are flattened. As a result, it's often useful to use a `Vec` to capture
    /// a "tail" of fields in a record:
    ///
    /// ```
    /// use std::error::Error;
    ///
    /// use csv::ReaderBuilder;
    /// use serde::Deserialize;
    ///
    /// #[derive(Debug, Deserialize, Eq, PartialEq)]
    /// struct Row {
    ///     label: String,
    ///     values: Vec<i32>,
    /// }
    ///
    /// # fn main() { example().unwrap(); }
    /// fn example() -> Result<(), Box<dyn Error>> {
    ///     let data = "foo,1,2,3";
    ///     let mut rdr = ReaderBuilder::new()
    ///         .has_headers(false)
    ///         .from_reader(data.as_bytes());
    ///     let mut iter = rdr.deserialize();
    ///
    ///     if let Some(result) = iter.next() {
    ///         let record: Row = result?;
    ///         assert_eq!(record, Row {
    ///             label: "foo".to_string(),
    ///             values: vec![1, 2, 3],
    ///         });
    ///         Ok(())
    ///     } else {
    ///         Err(From::from("expected at least one record but got none"))
    ///     }
    /// }
    /// ```
    ///
    /// In the above example, adding another field to the `Row` struct after
    /// the `values` field will result in a deserialization error. This is
    /// because the deserializer doesn't know when to stop reading fields
    /// into the `values` vector, so it will consume the rest of the fields in
    /// the record leaving none left over for the additional field.
    ///
    /// Finally, simple enums in Rust can be deserialized as well. Namely,
    /// enums must either be variants with no arguments or variants with a
    /// single argument. Variants with no arguments are deserialized based on
    /// which variant name the field matches. Variants with a single argument
    /// are deserialized based on which variant can store the data. The latter
    /// is only supported when using "untagged" enum deserialization. The
    /// following example shows both forms in action:
    ///
    /// ```
    /// use std::error::Error;
    ///
    /// use csv::Reader;
    /// use serde::Deserialize;
    ///
    /// #[derive(Debug, Deserialize, PartialEq)]
    /// struct Row {
    ///     label: Label,
    ///     value: Number,
    /// }
    ///
    /// #[derive(Debug, Deserialize, PartialEq)]
    /// #[serde(rename_all = "lowercase")]
    /// enum Label {
    ///     Celsius,
    ///     Fahrenheit,
    /// }
    ///
    /// #[derive(Debug, Deserialize, PartialEq)]
    /// #[serde(untagged)]
    /// enum Number {
    ///     Integer(i64),
    ///     Float(f64),
    /// }
    ///
    /// # fn main() { example().unwrap(); }
    /// fn example() -> Result<(), Box<dyn Error>> {
    ///     let data = "\
    /// label,value
    /// celsius,22.2222
    /// fahrenheit,72
    /// ";
    ///     let mut rdr = Reader::from_reader(data.as_bytes());
    ///     let mut iter = rdr.deserialize();
    ///
    ///     // Read the first record.
    ///     if let Some(result) = iter.next() {
    ///         let record: Row = result?;
    ///         assert_eq!(record, Row {
    ///             label: Label::Celsius,
    ///             value: Number::Float(22.2222),
    ///         });
    ///     } else {
    ///         return Err(From::from(
    ///             "expected at least two records but got none"));
    ///     }
    ///
    ///     // Read the second record.
    ///     if let Some(result) = iter.next() {
    ///         let record: Row = result?;
    ///         assert_eq!(record, Row {
    ///             label: Label::Fahrenheit,
    ///             value: Number::Integer(72),
    ///         });
    ///         Ok(())
    ///     } else {
    ///         Err(From::from(
    ///             "expected at least two records but got only one"))
    ///     }
    /// }
    /// ```
    pub fn deserialize<D>(&mut self) -> DeserializeRecordsIter<R, D>
    where
        D: DeserializeOwned,
    {
        DeserializeRecordsIter::new(self)
    }

    /// Returns an owned iterator over deserialized records.
    ///
    /// Each item yielded by this iterator is a `Result<D, Error>`.
    /// Therefore, in order to access the record, callers must handle the
    /// possibility of error (typically with `try!` or `?`).
    ///
    /// This is mostly useful when you want to return a CSV iterator or store
    /// it somewhere.
    ///
    /// If `has_headers` was enabled via a `ReaderBuilder` (which is the
    /// default), then this does not include the first record. Additionally,
    /// if `has_headers` is enabled, then deserializing into a struct will
    /// automatically align the values in each row to the fields of a struct
    /// based on the header row.
    ///
    /// For more detailed deserialization rules, see the documentation on the
    /// `deserialize` method.
    ///
    /// # Example
    ///
    /// ```
    /// use std::error::Error;
    ///
    /// use csv::Reader;
    /// use serde::Deserialize;
    ///
    /// #[derive(Debug, Deserialize, Eq, PartialEq)]
    /// struct Row {
    ///     city: String,
    ///     country: String,
    ///     #[serde(rename = "popcount")]
    ///     population: u64,
    /// }
    ///
    /// # fn main() { example().unwrap(); }
    /// fn example() -> Result<(), Box<dyn Error>> {
    ///     let data = "\
    /// city,country,popcount
    /// Boston,United States,4628910
    /// ";
    ///     let rdr = Reader::from_reader(data.as_bytes());
    ///     let mut iter = rdr.into_deserialize();
    ///
    ///     if let Some(result) = iter.next() {
    ///         let record: Row = result?;
    ///         assert_eq!(record, Row {
    ///             city: "Boston".to_string(),
    ///             country: "United States".to_string(),
    ///             population: 4628910,
    ///         });
    ///         Ok(())
    ///     } else {
    ///         Err(From::from("expected at least one record but got none"))
    ///     }
    /// }
    /// ```
    pub fn into_deserialize<D>(self) -> DeserializeRecordsIntoIter<R, D>
    where
        D: DeserializeOwned,
    {
        DeserializeRecordsIntoIter::new(self)
    }

    /// Returns a borrowed iterator over all records as strings.
    ///
    /// Each item yielded by this iterator is a `Result<StringRecord, Error>`.
    /// Therefore, in order to access the record, callers must handle the
    /// possibility of error (typically with `try!` or `?`).
    ///
    /// If `has_headers` was enabled via a `ReaderBuilder` (which is the
    /// default), then this does not include the first record.
    ///
    /// # Example
    ///
    /// ```
    /// use std::error::Error;
    /// use csv::Reader;
    ///
    /// # fn main() { example().unwrap(); }
    /// fn example() -> Result<(), Box<dyn Error>> {
    ///     let data = "\
    /// city,country,pop
    /// Boston,United States,4628910
    /// ";
    ///     let mut rdr = Reader::from_reader(data.as_bytes());
    ///     let mut iter = rdr.records();
    ///
    ///     if let Some(result) = iter.next() {
    ///         let record = result?;
    ///         assert_eq!(record, vec!["Boston", "United States", "4628910"]);
    ///         Ok(())
    ///     } else {
    ///         Err(From::from("expected at least one record but got none"))
    ///     }
    /// }
    /// ```
    pub fn records(&mut self) -> StringRecordsIter<R> {
        StringRecordsIter::new(self)
    }

    /// Returns an owned iterator over all records as strings.
    ///
    /// Each item yielded by this iterator is a `Result<StringRecord, Error>`.
    /// Therefore, in order to access the record, callers must handle the
    /// possibility of error (typically with `try!` or `?`).
    ///
    /// This is mostly useful when you want to return a CSV iterator or store
    /// it somewhere.
    ///
    /// If `has_headers` was enabled via a `ReaderBuilder` (which is the
    /// default), then this does not include the first record.
    ///
    /// # Example
    ///
    /// ```
    /// use std::error::Error;
    /// use csv::Reader;
    ///
    /// # fn main() { example().unwrap(); }
    /// fn example() -> Result<(), Box<dyn Error>> {
    ///     let data = "\
    /// city,country,pop
    /// Boston,United States,4628910
    /// ";
    ///     let rdr = Reader::from_reader(data.as_bytes());
    ///     let mut iter = rdr.into_records();
    ///
    ///     if let Some(result) = iter.next() {
    ///         let record = result?;
    ///         assert_eq!(record, vec!["Boston", "United States", "4628910"]);
    ///         Ok(())
    ///     } else {
    ///         Err(From::from("expected at least one record but got none"))
    ///     }
    /// }
    /// ```
    pub fn into_records(self) -> StringRecordsIntoIter<R> {
        StringRecordsIntoIter::new(self)
    }

    /// Returns a borrowed iterator over all records as raw bytes.
    ///
    /// Each item yielded by this iterator is a `Result<ByteRecord, Error>`.
    /// Therefore, in order to access the record, callers must handle the
    /// possibility of error (typically with `try!` or `?`).
    ///
    /// If `has_headers` was enabled via a `ReaderBuilder` (which is the
    /// default), then this does not include the first record.
    ///
    /// # Example
    ///
    /// ```
    /// use std::error::Error;
    /// use csv::Reader;
    ///
    /// # fn main() { example().unwrap(); }
    /// fn example() -> Result<(), Box<dyn Error>> {
    ///     let data = "\
    /// city,country,pop
    /// Boston,United States,4628910
    /// ";
    ///     let mut rdr = Reader::from_reader(data.as_bytes());
    ///     let mut iter = rdr.byte_records();
    ///
    ///     if let Some(result) = iter.next() {
    ///         let record = result?;
    ///         assert_eq!(record, vec!["Boston", "United States", "4628910"]);
    ///         Ok(())
    ///     } else {
    ///         Err(From::from("expected at least one record but got none"))
    ///     }
    /// }
    /// ```
    pub fn byte_records(&mut self) -> ByteRecordsIter<R> {
        ByteRecordsIter::new(self)
    }

    /// Returns an owned iterator over all records as raw bytes.
    ///
    /// Each item yielded by this iterator is a `Result<ByteRecord, Error>`.
    /// Therefore, in order to access the record, callers must handle the
    /// possibility of error (typically with `try!` or `?`).
    ///
    /// This is mostly useful when you want to return a CSV iterator or store
    /// it somewhere.
    ///
    /// If `has_headers` was enabled via a `ReaderBuilder` (which is the
    /// default), then this does not include the first record.
    ///
    /// # Example
    ///
    /// ```
    /// use std::error::Error;
    /// use csv::Reader;
    ///
    /// # fn main() { example().unwrap(); }
    /// fn example() -> Result<(), Box<dyn Error>> {
    ///     let data = "\
    /// city,country,pop
    /// Boston,United States,4628910
    /// ";
    ///     let rdr = Reader::from_reader(data.as_bytes());
    ///     let mut iter = rdr.into_byte_records();
    ///
    ///     if let Some(result) = iter.next() {
    ///         let record = result?;
    ///         assert_eq!(record, vec!["Boston", "United States", "4628910"]);
    ///         Ok(())
    ///     } else {
    ///         Err(From::from("expected at least one record but got none"))
    ///     }
    /// }
    /// ```
    pub fn into_byte_records(self) -> ByteRecordsIntoIter<R> {
        ByteRecordsIntoIter::new(self)
    }

    /// Returns a reference to the first row read by this parser.
    ///
    /// If no row has been read yet, then this will force parsing of the first
    /// row.
    ///
    /// If there was a problem parsing the row or if it wasn't valid UTF-8,
    /// then this returns an error.
    ///
    /// If the underlying reader emits EOF before any data, then this returns
    /// an empty record.
    ///
    /// Note that this method may be used regardless of whether `has_headers`
    /// was enabled (but it is enabled by default).
    ///
    /// # Example
    ///
    /// This example shows how to get the header row of CSV data. Notice that
    /// the header row does not appear as a record in the iterator!
    ///
    /// ```
    /// use std::error::Error;
    /// use csv::Reader;
    ///
    /// # fn main() { example().unwrap(); }
    /// fn example() -> Result<(), Box<dyn Error>> {
    ///     let data = "\
    /// city,country,pop
    /// Boston,United States,4628910
    /// ";
    ///     let mut rdr = Reader::from_reader(data.as_bytes());
    ///
    ///     // We can read the headers before iterating.
    ///     {
    ///         // `headers` borrows from the reader, so we put this in its
    ///         // own scope. That way, the borrow ends before we try iterating
    ///         // below. Alternatively, we could clone the headers.
    ///         let headers = rdr.headers()?;
    ///         assert_eq!(headers, vec!["city", "country", "pop"]);
    ///     }
    ///
    ///     if let Some(result) = rdr.records().next() {
    ///         let record = result?;
    ///         assert_eq!(record, vec!["Boston", "United States", "4628910"]);
    ///     } else {
    ///         return Err(From::from(
    ///             "expected at least one record but got none"))
    ///     }
    ///
    ///     // We can also read the headers after iterating.
    ///     let headers = rdr.headers()?;
    ///     assert_eq!(headers, vec!["city", "country", "pop"]);
    ///     Ok(())
    /// }
    /// ```
    pub fn headers(&mut self) -> Result<&StringRecord> {
        if self.state.headers.is_none() {
            let mut record = ByteRecord::new();
            self.read_byte_record_impl(&mut record)?;
            self.set_headers_impl(Err(record));
        }
        let headers = self.state.headers.as_ref().unwrap();
        match headers.string_record {
            Ok(ref record) => Ok(record),
            Err(ref err) => Err(Error::new(ErrorKind::Utf8 {
                pos: headers.byte_record.position().map(Clone::clone),
                err: err.clone(),
            })),
        }
    }

    /// Returns a reference to the first row read by this parser as raw bytes.
    ///
    /// If no row has been read yet, then this will force parsing of the first
    /// row.
    ///
    /// If there was a problem parsing the row then this returns an error.
    ///
    /// If the underlying reader emits EOF before any data, then this returns
    /// an empty record.
    ///
    /// Note that this method may be used regardless of whether `has_headers`
    /// was enabled (but it is enabled by default).
    ///
    /// # Example
    ///
    /// This example shows how to get the header row of CSV data. Notice that
    /// the header row does not appear as a record in the iterator!
    ///
    /// ```
    /// use std::error::Error;
    /// use csv::Reader;
    ///
    /// # fn main() { example().unwrap(); }
    /// fn example() -> Result<(), Box<dyn Error>> {
    ///     let data = "\
    /// city,country,pop
    /// Boston,United States,4628910
    /// ";
    ///     let mut rdr = Reader::from_reader(data.as_bytes());
    ///
    ///     // We can read the headers before iterating.
    ///     {
    ///         // `headers` borrows from the reader, so we put this in its
    ///         // own scope. That way, the borrow ends before we try iterating
    ///         // below. Alternatively, we could clone the headers.
    ///         let headers = rdr.byte_headers()?;
    ///         assert_eq!(headers, vec!["city", "country", "pop"]);
    ///     }
    ///
    ///     if let Some(result) = rdr.byte_records().next() {
    ///         let record = result?;
    ///         assert_eq!(record, vec!["Boston", "United States", "4628910"]);
    ///     } else {
    ///         return Err(From::from(
    ///             "expected at least one record but got none"))
    ///     }
    ///
    ///     // We can also read the headers after iterating.
    ///     let headers = rdr.byte_headers()?;
    ///     assert_eq!(headers, vec!["city", "country", "pop"]);
    ///     Ok(())
    /// }
    /// ```
    pub fn byte_headers(&mut self) -> Result<&ByteRecord> {
        if self.state.headers.is_none() {
            let mut record = ByteRecord::new();
            self.read_byte_record_impl(&mut record)?;
            self.set_headers_impl(Err(record));
        }
        Ok(&self.state.headers.as_ref().unwrap().byte_record)
    }

    /// Set the headers of this CSV parser manually.
    ///
    /// This overrides any other setting (including `set_byte_headers`). Any
    /// automatic detection of headers is disabled. This may be called at any
    /// time.
    ///
    /// # Example
    ///
    /// ```
    /// use std::error::Error;
    /// use csv::{Reader, StringRecord};
    ///
    /// # fn main() { example().unwrap(); }
    /// fn example() -> Result<(), Box<dyn Error>> {
    ///     let data = "\
    /// city,country,pop
    /// Boston,United States,4628910
    /// ";
    ///     let mut rdr = Reader::from_reader(data.as_bytes());
    ///
    ///     assert_eq!(rdr.headers()?, vec!["city", "country", "pop"]);
    ///     rdr.set_headers(StringRecord::from(vec!["a", "b", "c"]));
    ///     assert_eq!(rdr.headers()?, vec!["a", "b", "c"]);
    ///
    ///     Ok(())
    /// }
    /// ```
    pub fn set_headers(&mut self, headers: StringRecord) {
        self.set_headers_impl(Ok(headers));
    }

    /// Set the headers of this CSV parser manually as raw bytes.
    ///
    /// This overrides any other setting (including `set_headers`). Any
    /// automatic detection of headers is disabled. This may be called at any
    /// time.
    ///
    /// # Example
    ///
    /// ```
    /// use std::error::Error;
    /// use csv::{Reader, ByteRecord};
    ///
    /// # fn main() { example().unwrap(); }
    /// fn example() -> Result<(), Box<dyn Error>> {
    ///     let data = "\
    /// city,country,pop
    /// Boston,United States,4628910
    /// ";
    ///     let mut rdr = Reader::from_reader(data.as_bytes());
    ///
    ///     assert_eq!(rdr.byte_headers()?, vec!["city", "country", "pop"]);
    ///     rdr.set_byte_headers(ByteRecord::from(vec!["a", "b", "c"]));
    ///     assert_eq!(rdr.byte_headers()?, vec!["a", "b", "c"]);
    ///
    ///     Ok(())
    /// }
    /// ```
    pub fn set_byte_headers(&mut self, headers: ByteRecord) {
        self.set_headers_impl(Err(headers));
    }

    fn set_headers_impl(
        &mut self,
        headers: result::Result<StringRecord, ByteRecord>,
    ) {
        // If we have string headers, then get byte headers. But if we have
        // byte headers, then get the string headers (or a UTF-8 error).
        let (mut str_headers, mut byte_headers) = match headers {
            Ok(string) => {
                let bytes = string.clone().into_byte_record();
                (Ok(string), bytes)
            }
            Err(bytes) => {
                match StringRecord::from_byte_record(bytes.clone()) {
                    Ok(str_headers) => (Ok(str_headers), bytes),
                    Err(err) => (Err(err.utf8_error().clone()), bytes),
                }
            }
        };
        if self.state.trim.should_trim_headers() {
            if let Ok(ref mut str_headers) = str_headers.as_mut() {
                str_headers.trim();
            }
            byte_headers.trim();
        }
        self.state.headers = Some(Headers {
            byte_record: byte_headers,
            string_record: str_headers,
        });
    }

    /// Read a single row into the given record. Returns false when no more
    /// records could be read.
    ///
    /// If `has_headers` was enabled via a `ReaderBuilder` (which is the
    /// default), then this will never read the first record.
    ///
    /// This method is useful when you want to read records as fast as
    /// as possible. It's less ergonomic than an iterator, but it permits the
    /// caller to reuse the `StringRecord` allocation, which usually results
    /// in higher throughput.
    ///
    /// Records read via this method are guaranteed to have a position set
    /// on them, even if the reader is at EOF or if an error is returned.
    ///
    /// # Example
    ///
    /// ```
    /// use std::error::Error;
    /// use csv::{Reader, StringRecord};
    ///
    /// # fn main() { example().unwrap(); }
    /// fn example() -> Result<(), Box<dyn Error>> {
    ///     let data = "\
    /// city,country,pop
    /// Boston,United States,4628910
    /// ";
    ///     let mut rdr = Reader::from_reader(data.as_bytes());
    ///     let mut record = StringRecord::new();
    ///
    ///     if rdr.read_record(&mut record)? {
    ///         assert_eq!(record, vec!["Boston", "United States", "4628910"]);
    ///         Ok(())
    ///     } else {
    ///         Err(From::from("expected at least one record but got none"))
    ///     }
    /// }
    /// ```
    pub fn read_record(&mut self, record: &mut StringRecord) -> Result<bool> {
        let result = record.read(self);
        // We need to trim again because trimming string records includes
        // Unicode whitespace. (ByteRecord trimming only includes ASCII
        // whitespace.)
        if self.state.trim.should_trim_fields() {
            record.trim();
        }
        result
    }

    /// Read a single row into the given byte record. Returns false when no
    /// more records could be read.
    ///
    /// If `has_headers` was enabled via a `ReaderBuilder` (which is the
    /// default), then this will never read the first record.
    ///
    /// This method is useful when you want to read records as fast as
    /// as possible. It's less ergonomic than an iterator, but it permits the
    /// caller to reuse the `ByteRecord` allocation, which usually results
    /// in higher throughput.
    ///
    /// Records read via this method are guaranteed to have a position set
    /// on them, even if the reader is at EOF or if an error is returned.
    ///
    /// # Example
    ///
    /// ```
    /// use std::error::Error;
    /// use csv::{ByteRecord, Reader};
    ///
    /// # fn main() { example().unwrap(); }
    /// fn example() -> Result<(), Box<dyn Error>> {
    ///     let data = "\
    /// city,country,pop
    /// Boston,United States,4628910
    /// ";
    ///     let mut rdr = Reader::from_reader(data.as_bytes());
    ///     let mut record = ByteRecord::new();
    ///
    ///     if rdr.read_byte_record(&mut record)? {
    ///         assert_eq!(record, vec!["Boston", "United States", "4628910"]);
    ///         Ok(())
    ///     } else {
    ///         Err(From::from("expected at least one record but got none"))
    ///     }
    /// }
    /// ```
    pub fn read_byte_record(
        &mut self,
        record: &mut ByteRecord,
    ) -> Result<bool> {
        if !self.state.seeked && !self.state.has_headers && !self.state.first {
            // If the caller indicated "no headers" and we haven't yielded the
            // first record yet, then we should yield our header row if we have
            // one.
            if let Some(ref headers) = self.state.headers {
                self.state.first = true;
                record.clone_from(&headers.byte_record);
                if self.state.trim.should_trim_fields() {
                    record.trim();
                }
                return Ok(!record.is_empty());
            }
        }
        let ok = self.read_byte_record_impl(record)?;
        self.state.first = true;
        if !self.state.seeked && self.state.headers.is_none() {
            self.set_headers_impl(Err(record.clone()));
            // If the end user indicated that we have headers, then we should
            // never return the first row. Instead, we should attempt to
            // read and return the next one.
            if self.state.has_headers {
                let result = self.read_byte_record_impl(record);
                if self.state.trim.should_trim_fields() {
                    record.trim();
                }
                return result;
            }
        } else if self.state.trim.should_trim_fields() {
            record.trim();
        }
        Ok(ok)
    }

    /// Read a byte record from the underlying CSV reader, without accounting
    /// for headers.
    #[inline(always)]
    fn read_byte_record_impl(
        &mut self,
        record: &mut ByteRecord,
    ) -> Result<bool> {
        use csv_core::ReadRecordResult::*;

        record.clear();
        record.set_position(Some(self.state.cur_pos.clone()));
        if self.state.eof != ReaderEofState::NotEof {
            return Ok(false);
        }
        let (mut outlen, mut endlen) = (0, 0);
        loop {
            let (res, nin, nout, nend) = {
                let input_res = self.rdr.fill_buf();
                if input_res.is_err() {
                    self.state.eof = ReaderEofState::IOError;
                }
                let input = input_res?;
                let (fields, ends) = record.as_parts();
                self.core.read_record(
                    input,
                    &mut fields[outlen..],
                    &mut ends[endlen..],
                )
            };
            self.rdr.consume(nin);
            let byte = self.state.cur_pos.byte();
            self.state
                .cur_pos
                .set_byte(byte + nin as u64)
                .set_line(self.core.line());
            outlen += nout;
            endlen += nend;
            match res {
                InputEmpty => continue,
                OutputFull => {
                    record.expand_fields();
                    continue;
                }
                OutputEndsFull => {
                    record.expand_ends();
                    continue;
                }
                Record => {
                    record.set_len(endlen);
                    self.state.add_record(record)?;
                    return Ok(true);
                }
                End => {
                    self.state.eof = ReaderEofState::Eof;
                    return Ok(false);
                }
            }
        }
    }

    /// Return the current position of this CSV reader.
    ///
    /// The byte offset in the position returned can be used to `seek` this
    /// reader. In particular, seeking to a position returned here on the same
    /// data will result in parsing the same subsequent record.
    ///
    /// # Example: reading the position
    ///
    /// ```
    /// use std::error::Error;
    /// use std::io;
    /// use csv::{Reader, Position};
    ///
    /// # fn main() { example().unwrap(); }
    /// fn example() -> Result<(), Box<dyn Error>> {
    ///     let data = "\
    /// city,country,popcount
    /// Boston,United States,4628910
    /// Concord,United States,42695
    /// ";
    ///     let rdr = Reader::from_reader(io::Cursor::new(data));
    ///     let mut iter = rdr.into_records();
    ///     let mut pos = Position::new();
    ///     loop {
    ///         // Read the position immediately before each record.
    ///         let next_pos = iter.reader().position().clone();
    ///         if iter.next().is_none() {
    ///             break;
    ///         }
    ///         pos = next_pos;
    ///     }
    ///
    ///     // `pos` should now be the position immediately before the last
    ///     // record.
    ///     assert_eq!(pos.byte(), 51);
    ///     assert_eq!(pos.line(), 3);
    ///     assert_eq!(pos.record(), 2);
    ///     Ok(())
    /// }
    /// ```
    pub fn position(&self) -> &Position {
        &self.state.cur_pos
    }

    /// Returns true if and only if this reader has been exhausted.
    ///
    /// When this returns true, no more records can be read from this reader
    /// (unless it has been seeked to another position).
    ///
    /// # Example
    ///
    /// ```
    /// use std::error::Error;
    /// use std::io;
    /// use csv::{Reader, Position};
    ///
    /// # fn main() { example().unwrap(); }
    /// fn example() -> Result<(), Box<dyn Error>> {
    ///     let data = "\
    /// city,country,popcount
    /// Boston,United States,4628910
    /// Concord,United States,42695
    /// ";
    ///     let mut rdr = Reader::from_reader(io::Cursor::new(data));
    ///     assert!(!rdr.is_done());
    ///     for result in rdr.records() {
    ///         let _ = result?;
    ///     }
    ///     assert!(rdr.is_done());
    ///     Ok(())
    /// }
    /// ```
    pub fn is_done(&self) -> bool {
        self.state.eof != ReaderEofState::NotEof
    }

    /// Returns true if and only if this reader has been configured to
    /// interpret the first record as a header record.
    pub fn has_headers(&self) -> bool {
        self.state.has_headers
    }

    /// Returns a reference to the underlying reader.
    pub fn get_ref(&self) -> &R {
        self.rdr.get_ref()
    }

    /// Returns a mutable reference to the underlying reader.
    pub fn get_mut(&mut self) -> &mut R {
        self.rdr.get_mut()
    }

    /// Unwraps this CSV reader, returning the underlying reader.
    ///
    /// Note that any leftover data inside this reader's internal buffer is
    /// lost.
    pub fn into_inner(self) -> R {
        self.rdr.into_inner()
    }
}

impl<R: io::Read + io::Seek> Reader<R> {
    /// Seeks the underlying reader to the position given.
    ///
    /// This comes with a few caveats:
    ///
    /// * Any internal buffer associated with this reader is cleared.
    /// * If the given position does not correspond to a position immediately
    ///   before the start of a record, then the behavior of this reader is
    ///   unspecified.
    /// * Any special logic that skips the first record in the CSV reader
    ///   when reading or iterating over records is disabled.
    ///
    /// If the given position has a byte offset equivalent to the current
    /// position, then no seeking is performed.
    ///
    /// If the header row has not already been read, then this will attempt
    /// to read the header row before seeking. Therefore, it is possible that
    /// this returns an error associated with reading CSV data.
    ///
    /// Note that seeking is performed based only on the byte offset in the
    /// given position. Namely, the record or line numbers in the position may
    /// be incorrect, but this will cause any future position generated by
    /// this CSV reader to be similarly incorrect.
    ///
    /// # Example: seek to parse a record twice
    ///
    /// ```
    /// use std::error::Error;
    /// use std::io;
    /// use csv::{Reader, Position};
    ///
    /// # fn main() { example().unwrap(); }
    /// fn example() -> Result<(), Box<dyn Error>> {
    ///     let data = "\
    /// city,country,popcount
    /// Boston,United States,4628910
    /// Concord,United States,42695
    /// ";
    ///     let rdr = Reader::from_reader(io::Cursor::new(data));
    ///     let mut iter = rdr.into_records();
    ///     let mut pos = Position::new();
    ///     loop {
    ///         // Read the position immediately before each record.
    ///         let next_pos = iter.reader().position().clone();
    ///         if iter.next().is_none() {
    ///             break;
    ///         }
    ///         pos = next_pos;
    ///     }
    ///
    ///     // Now seek the reader back to `pos`. This will let us read the
    ///     // last record again.
    ///     iter.reader_mut().seek(pos)?;
    ///     let mut iter = iter.into_reader().into_records();
    ///     if let Some(result) = iter.next() {
    ///         let record = result?;
    ///         assert_eq!(record, vec!["Concord", "United States", "42695"]);
    ///         Ok(())
    ///     } else {
    ///         Err(From::from("expected at least one record but got none"))
    ///     }
    /// }
    /// ```
    pub fn seek(&mut self, pos: Position) -> Result<()> {
        self.byte_headers()?;
        self.state.seeked = true;
        if pos.byte() == self.state.cur_pos.byte() {
            return Ok(());
        }
        self.rdr.seek(io::SeekFrom::Start(pos.byte()))?;
        self.core.reset();
        self.core.set_line(pos.line());
        self.state.cur_pos = pos;
        self.state.eof = ReaderEofState::NotEof;
        Ok(())
    }

    /// This is like `seek`, but provides direct control over how the seeking
    /// operation is performed via `io::SeekFrom`.
    ///
    /// The `pos` position given *should* correspond the position indicated
    /// by `seek_from`, but there is no requirement. If the `pos` position
    /// given is incorrect, then the position information returned by this
    /// reader will be similarly incorrect.
    ///
    /// If the header row has not already been read, then this will attempt
    /// to read the header row before seeking. Therefore, it is possible that
    /// this returns an error associated with reading CSV data.
    ///
    /// Unlike `seek`, this will always cause an actual seek to be performed.
    pub fn seek_raw(
        &mut self,
        seek_from: io::SeekFrom,
        pos: Position,
    ) -> Result<()> {
        self.byte_headers()?;
        self.state.seeked = true;
        self.rdr.seek(seek_from)?;
        self.core.reset();
        self.core.set_line(pos.line());
        self.state.cur_pos = pos;
        self.state.eof = ReaderEofState::NotEof;
        Ok(())
    }
}

impl ReaderState {
    #[inline(always)]
    fn add_record(&mut self, record: &ByteRecord) -> Result<()> {
        let i = self.cur_pos.record();
        self.cur_pos.set_record(i.checked_add(1).unwrap());
        if !self.flexible {
            match self.first_field_count {
                None => self.first_field_count = Some(record.len() as u64),
                Some(expected) => {
                    if record.len() as u64 != expected {
                        return Err(Error::new(ErrorKind::UnequalLengths {
                            pos: record.position().map(Clone::clone),
                            expected_len: expected,
                            len: record.len() as u64,
                        }));
                    }
                }
            }
        }
        Ok(())
    }
}

/// An owned iterator over deserialized records.
///
/// The type parameter `R` refers to the underlying `io::Read` type, and `D`
/// refers to the type that this iterator will deserialize a record into.
pub struct DeserializeRecordsIntoIter<R, D> {
    rdr: Reader<R>,
    rec: StringRecord,
    headers: Option<StringRecord>,
    _priv: PhantomData<D>,
}

impl<R: io::Read, D: DeserializeOwned> DeserializeRecordsIntoIter<R, D> {
    fn new(mut rdr: Reader<R>) -> DeserializeRecordsIntoIter<R, D> {
        let headers = if !rdr.state.has_headers {
            None
        } else {
            rdr.headers().ok().map(Clone::clone)
        };
        DeserializeRecordsIntoIter {
            rdr: rdr,
            rec: StringRecord::new(),
            headers: headers,
            _priv: PhantomData,
        }
    }

    /// Return a reference to the underlying CSV reader.
    pub fn reader(&self) -> &Reader<R> {
        &self.rdr
    }

    /// Return a mutable reference to the underlying CSV reader.
    pub fn reader_mut(&mut self) -> &mut Reader<R> {
        &mut self.rdr
    }

    /// Drop this iterator and return the underlying CSV reader.
    pub fn into_reader(self) -> Reader<R> {
        self.rdr
    }
}

impl<R: io::Read, D: DeserializeOwned> Iterator
    for DeserializeRecordsIntoIter<R, D>
{
    type Item = Result<D>;

    fn next(&mut self) -> Option<Result<D>> {
        match self.rdr.read_record(&mut self.rec) {
            Err(err) => Some(Err(err)),
            Ok(false) => None,
            Ok(true) => Some(self.rec.deserialize(self.headers.as_ref())),
        }
    }
}

/// A borrowed iterator over deserialized records.
///
/// The lifetime parameter `'r` refers to the lifetime of the underlying
/// CSV `Reader`. The type parameter `R` refers to the underlying `io::Read`
/// type, and `D` refers to the type that this iterator will deserialize a
/// record into.
pub struct DeserializeRecordsIter<'r, R: 'r, D> {
    rdr: &'r mut Reader<R>,
    rec: StringRecord,
    headers: Option<StringRecord>,
    _priv: PhantomData<D>,
}

impl<'r, R: io::Read, D: DeserializeOwned> DeserializeRecordsIter<'r, R, D> {
    fn new(rdr: &'r mut Reader<R>) -> DeserializeRecordsIter<'r, R, D> {
        let headers = if !rdr.state.has_headers {
            None
        } else {
            rdr.headers().ok().map(Clone::clone)
        };
        DeserializeRecordsIter {
            rdr: rdr,
            rec: StringRecord::new(),
            headers: headers,
            _priv: PhantomData,
        }
    }

    /// Return a reference to the underlying CSV reader.
    pub fn reader(&self) -> &Reader<R> {
        &self.rdr
    }

    /// Return a mutable reference to the underlying CSV reader.
    pub fn reader_mut(&mut self) -> &mut Reader<R> {
        &mut self.rdr
    }
}

impl<'r, R: io::Read, D: DeserializeOwned> Iterator
    for DeserializeRecordsIter<'r, R, D>
{
    type Item = Result<D>;

    fn next(&mut self) -> Option<Result<D>> {
        match self.rdr.read_record(&mut self.rec) {
            Err(err) => Some(Err(err)),
            Ok(false) => None,
            Ok(true) => Some(self.rec.deserialize(self.headers.as_ref())),
        }
    }
}

/// An owned iterator over records as strings.
pub struct StringRecordsIntoIter<R> {
    rdr: Reader<R>,
    rec: StringRecord,
}

impl<R: io::Read> StringRecordsIntoIter<R> {
    fn new(rdr: Reader<R>) -> StringRecordsIntoIter<R> {
        StringRecordsIntoIter { rdr: rdr, rec: StringRecord::new() }
    }

    /// Return a reference to the underlying CSV reader.
    pub fn reader(&self) -> &Reader<R> {
        &self.rdr
    }

    /// Return a mutable reference to the underlying CSV reader.
    pub fn reader_mut(&mut self) -> &mut Reader<R> {
        &mut self.rdr
    }

    /// Drop this iterator and return the underlying CSV reader.
    pub fn into_reader(self) -> Reader<R> {
        self.rdr
    }
}

impl<R: io::Read> Iterator for StringRecordsIntoIter<R> {
    type Item = Result<StringRecord>;

    fn next(&mut self) -> Option<Result<StringRecord>> {
        match self.rdr.read_record(&mut self.rec) {
            Err(err) => Some(Err(err)),
            Ok(true) => Some(Ok(self.rec.clone_truncated())),
            Ok(false) => None,
        }
    }
}

/// A borrowed iterator over records as strings.
///
/// The lifetime parameter `'r` refers to the lifetime of the underlying
/// CSV `Reader`.
pub struct StringRecordsIter<'r, R: 'r> {
    rdr: &'r mut Reader<R>,
    rec: StringRecord,
}

impl<'r, R: io::Read> StringRecordsIter<'r, R> {
    fn new(rdr: &'r mut Reader<R>) -> StringRecordsIter<'r, R> {
        StringRecordsIter { rdr: rdr, rec: StringRecord::new() }
    }

    /// Return a reference to the underlying CSV reader.
    pub fn reader(&self) -> &Reader<R> {
        &self.rdr
    }

    /// Return a mutable reference to the underlying CSV reader.
    pub fn reader_mut(&mut self) -> &mut Reader<R> {
        &mut self.rdr
    }
}

impl<'r, R: io::Read> Iterator for StringRecordsIter<'r, R> {
    type Item = Result<StringRecord>;

    fn next(&mut self) -> Option<Result<StringRecord>> {
        match self.rdr.read_record(&mut self.rec) {
            Err(err) => Some(Err(err)),
            Ok(true) => Some(Ok(self.rec.clone_truncated())),
            Ok(false) => None,
        }
    }
}

/// An owned iterator over records as raw bytes.
pub struct ByteRecordsIntoIter<R> {
    rdr: Reader<R>,
    rec: ByteRecord,
}

impl<R: io::Read> ByteRecordsIntoIter<R> {
    fn new(rdr: Reader<R>) -> ByteRecordsIntoIter<R> {
        ByteRecordsIntoIter { rdr: rdr, rec: ByteRecord::new() }
    }

    /// Return a reference to the underlying CSV reader.
    pub fn reader(&self) -> &Reader<R> {
        &self.rdr
    }

    /// Return a mutable reference to the underlying CSV reader.
    pub fn reader_mut(&mut self) -> &mut Reader<R> {
        &mut self.rdr
    }

    /// Drop this iterator and return the underlying CSV reader.
    pub fn into_reader(self) -> Reader<R> {
        self.rdr
    }
}

impl<R: io::Read> Iterator for ByteRecordsIntoIter<R> {
    type Item = Result<ByteRecord>;

    fn next(&mut self) -> Option<Result<ByteRecord>> {
        match self.rdr.read_byte_record(&mut self.rec) {
            Err(err) => Some(Err(err)),
            Ok(true) => Some(Ok(self.rec.clone_truncated())),
            Ok(false) => None,
        }
    }
}

/// A borrowed iterator over records as raw bytes.
///
/// The lifetime parameter `'r` refers to the lifetime of the underlying
/// CSV `Reader`.
pub struct ByteRecordsIter<'r, R: 'r> {
    rdr: &'r mut Reader<R>,
    rec: ByteRecord,
}

impl<'r, R: io::Read> ByteRecordsIter<'r, R> {
    fn new(rdr: &'r mut Reader<R>) -> ByteRecordsIter<'r, R> {
        ByteRecordsIter { rdr: rdr, rec: ByteRecord::new() }
    }

    /// Return a reference to the underlying CSV reader.
    pub fn reader(&self) -> &Reader<R> {
        &self.rdr
    }

    /// Return a mutable reference to the underlying CSV reader.
    pub fn reader_mut(&mut self) -> &mut Reader<R> {
        &mut self.rdr
    }
}

impl<'r, R: io::Read> Iterator for ByteRecordsIter<'r, R> {
    type Item = Result<ByteRecord>;

    fn next(&mut self) -> Option<Result<ByteRecord>> {
        match self.rdr.read_byte_record(&mut self.rec) {
            Err(err) => Some(Err(err)),
            Ok(true) => Some(Ok(self.rec.clone_truncated())),
            Ok(false) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io;

    use crate::byte_record::ByteRecord;
    use crate::error::ErrorKind;
    use crate::string_record::StringRecord;

    use super::{Position, ReaderBuilder, Trim};

    fn b(s: &str) -> &[u8] {
        s.as_bytes()
    }
    fn s(b: &[u8]) -> &str {
        ::std::str::from_utf8(b).unwrap()
    }

    fn newpos(byte: u64, line: u64, record: u64) -> Position {
        let mut p = Position::new();
        p.set_byte(byte).set_line(line).set_record(record);
        p
    }

    #[test]
    fn read_byte_record() {
        let data = b("foo,\"b,ar\",baz\nabc,mno,xyz");
        let mut rdr =
            ReaderBuilder::new().has_headers(false).from_reader(data);
        let mut rec = ByteRecord::new();

        assert!(rdr.read_byte_record(&mut rec).unwrap());
        assert_eq!(3, rec.len());
        assert_eq!("foo", s(&rec[0]));
        assert_eq!("b,ar", s(&rec[1]));
        assert_eq!("baz", s(&rec[2]));

        assert!(rdr.read_byte_record(&mut rec).unwrap());
        assert_eq!(3, rec.len());
        assert_eq!("abc", s(&rec[0]));
        assert_eq!("mno", s(&rec[1]));
        assert_eq!("xyz", s(&rec[2]));

        assert!(!rdr.read_byte_record(&mut rec).unwrap());
    }

    #[test]
    fn read_trimmed_records_and_headers() {
        let data = b("foo,  bar,\tbaz\n  1,  2,  3\n1\t,\t,3\t\t");
        let mut rdr = ReaderBuilder::new()
            .has_headers(true)
            .trim(Trim::All)
            .from_reader(data);
        let mut rec = ByteRecord::new();
        assert!(rdr.read_byte_record(&mut rec).unwrap());
        assert_eq!("1", s(&rec[0]));
        assert_eq!("2", s(&rec[1]));
        assert_eq!("3", s(&rec[2]));
        let mut rec = StringRecord::new();
        assert!(rdr.read_record(&mut rec).unwrap());
        assert_eq!("1", &rec[0]);
        assert_eq!("", &rec[1]);
        assert_eq!("3", &rec[2]);
        {
            let headers = rdr.headers().unwrap();
            assert_eq!(3, headers.len());
            assert_eq!("foo", &headers[0]);
            assert_eq!("bar", &headers[1]);
            assert_eq!("baz", &headers[2]);
        }
    }

    #[test]
    fn read_trimmed_header() {
        let data = b("foo,  bar,\tbaz\n  1,  2,  3\n1\t,\t,3\t\t");
        let mut rdr = ReaderBuilder::new()
            .has_headers(true)
            .trim(Trim::Headers)
            .from_reader(data);
        let mut rec = ByteRecord::new();
        assert!(rdr.read_byte_record(&mut rec).unwrap());
        assert_eq!("  1", s(&rec[0]));
        assert_eq!("  2", s(&rec[1]));
        assert_eq!("  3", s(&rec[2]));
        {
            let headers = rdr.headers().unwrap();
            assert_eq!(3, headers.len());
            assert_eq!("foo", &headers[0]);
            assert_eq!("bar", &headers[1]);
            assert_eq!("baz", &headers[2]);
        }
    }

    #[test]
    fn read_trimed_header_invalid_utf8() {
        let data = &b"foo,  b\xFFar,\tbaz\na,b,c\nd,e,f"[..];
        let mut rdr = ReaderBuilder::new()
            .has_headers(true)
            .trim(Trim::Headers)
            .from_reader(data);
        let mut rec = StringRecord::new();

        // force the headers to be read
        let _ = rdr.read_record(&mut rec);
        // Check the byte headers are trimmed
        {
            let headers = rdr.byte_headers().unwrap();
            assert_eq!(3, headers.len());
            assert_eq!(b"foo", &headers[0]);
            assert_eq!(b"b\xFFar", &headers[1]);
            assert_eq!(b"baz", &headers[2]);
        }
        match *rdr.headers().unwrap_err().kind() {
            ErrorKind::Utf8 { pos: Some(ref pos), ref err } => {
                assert_eq!(pos, &newpos(0, 1, 0));
                assert_eq!(err.field(), 1);
                assert_eq!(err.valid_up_to(), 3);
            }
            ref err => panic!("match failed, got {:?}", err),
        }
    }

    #[test]
    fn read_trimmed_records() {
        let data = b("foo,  bar,\tbaz\n  1,  2,  3\n1\t,\t,3\t\t");
        let mut rdr = ReaderBuilder::new()
            .has_headers(true)
            .trim(Trim::Fields)
            .from_reader(data);
        let mut rec = ByteRecord::new();
        assert!(rdr.read_byte_record(&mut rec).unwrap());
        assert_eq!("1", s(&rec[0]));
        assert_eq!("2", s(&rec[1]));
        assert_eq!("3", s(&rec[2]));
        {
            let headers = rdr.headers().unwrap();
            assert_eq!(3, headers.len());
            assert_eq!("foo", &headers[0]);
            assert_eq!("  bar", &headers[1]);
            assert_eq!("\tbaz", &headers[2]);
        }
    }

    #[test]
    fn read_record_unequal_fails() {
        let data = b("foo\nbar,baz");
        let mut rdr =
            ReaderBuilder::new().has_headers(false).from_reader(data);
        let mut rec = ByteRecord::new();

        assert!(rdr.read_byte_record(&mut rec).unwrap());
        assert_eq!(1, rec.len());
        assert_eq!("foo", s(&rec[0]));

        match rdr.read_byte_record(&mut rec) {
            Err(err) => match *err.kind() {
                ErrorKind::UnequalLengths {
                    expected_len: 1,
                    ref pos,
                    len: 2,
                } => {
                    assert_eq!(pos, &Some(newpos(4, 2, 1)));
                }
                ref wrong => panic!("match failed, got {:?}", wrong),
            },
            wrong => panic!("match failed, got {:?}", wrong),
        }
    }

    #[test]
    fn read_record_unequal_ok() {
        let data = b("foo\nbar,baz");
        let mut rdr = ReaderBuilder::new()
            .has_headers(false)
            .flexible(true)
            .from_reader(data);
        let mut rec = ByteRecord::new();

        assert!(rdr.read_byte_record(&mut rec).unwrap());
        assert_eq!(1, rec.len());
        assert_eq!("foo", s(&rec[0]));

        assert!(rdr.read_byte_record(&mut rec).unwrap());
        assert_eq!(2, rec.len());
        assert_eq!("bar", s(&rec[0]));
        assert_eq!("baz", s(&rec[1]));

        assert!(!rdr.read_byte_record(&mut rec).unwrap());
    }

    // This tests that even if we get a CSV error, we can continue reading
    // if we want.
    #[test]
    fn read_record_unequal_continue() {
        let data = b("foo\nbar,baz\nquux");
        let mut rdr =
            ReaderBuilder::new().has_headers(false).from_reader(data);
        let mut rec = ByteRecord::new();

        assert!(rdr.read_byte_record(&mut rec).unwrap());
        assert_eq!(1, rec.len());
        assert_eq!("foo", s(&rec[0]));

        match rdr.read_byte_record(&mut rec) {
            Err(err) => match err.kind() {
                &ErrorKind::UnequalLengths {
                    expected_len: 1,
                    ref pos,
                    len: 2,
                } => {
                    assert_eq!(pos, &Some(newpos(4, 2, 1)));
                }
                wrong => panic!("match failed, got {:?}", wrong),
            },
            wrong => panic!("match failed, got {:?}", wrong),
        }

        assert!(rdr.read_byte_record(&mut rec).unwrap());
        assert_eq!(1, rec.len());
        assert_eq!("quux", s(&rec[0]));

        assert!(!rdr.read_byte_record(&mut rec).unwrap());
    }

    #[test]
    fn read_record_headers() {
        let data = b("foo,bar,baz\na,b,c\nd,e,f");
        let mut rdr = ReaderBuilder::new().has_headers(true).from_reader(data);
        let mut rec = StringRecord::new();

        assert!(rdr.read_record(&mut rec).unwrap());
        assert_eq!(3, rec.len());
        assert_eq!("a", &rec[0]);

        assert!(rdr.read_record(&mut rec).unwrap());
        assert_eq!(3, rec.len());
        assert_eq!("d", &rec[0]);

        assert!(!rdr.read_record(&mut rec).unwrap());

        {
            let headers = rdr.byte_headers().unwrap();
            assert_eq!(3, headers.len());
            assert_eq!(b"foo", &headers[0]);
            assert_eq!(b"bar", &headers[1]);
            assert_eq!(b"baz", &headers[2]);
        }
        {
            let headers = rdr.headers().unwrap();
            assert_eq!(3, headers.len());
            assert_eq!("foo", &headers[0]);
            assert_eq!("bar", &headers[1]);
            assert_eq!("baz", &headers[2]);
        }
    }

    #[test]
    fn read_record_headers_invalid_utf8() {
        let data = &b"foo,b\xFFar,baz\na,b,c\nd,e,f"[..];
        let mut rdr = ReaderBuilder::new().has_headers(true).from_reader(data);
        let mut rec = StringRecord::new();

        assert!(rdr.read_record(&mut rec).unwrap());
        assert_eq!(3, rec.len());
        assert_eq!("a", &rec[0]);

        assert!(rdr.read_record(&mut rec).unwrap());
        assert_eq!(3, rec.len());
        assert_eq!("d", &rec[0]);

        assert!(!rdr.read_record(&mut rec).unwrap());

        // Check that we can read the headers as raw bytes, but that
        // if we read them as strings, we get an appropriate UTF-8 error.
        {
            let headers = rdr.byte_headers().unwrap();
            assert_eq!(3, headers.len());
            assert_eq!(b"foo", &headers[0]);
            assert_eq!(b"b\xFFar", &headers[1]);
            assert_eq!(b"baz", &headers[2]);
        }
        match *rdr.headers().unwrap_err().kind() {
            ErrorKind::Utf8 { pos: Some(ref pos), ref err } => {
                assert_eq!(pos, &newpos(0, 1, 0));
                assert_eq!(err.field(), 1);
                assert_eq!(err.valid_up_to(), 1);
            }
            ref err => panic!("match failed, got {:?}", err),
        }
    }

    #[test]
    fn read_record_no_headers_before() {
        let data = b("foo,bar,baz\na,b,c\nd,e,f");
        let mut rdr =
            ReaderBuilder::new().has_headers(false).from_reader(data);
        let mut rec = StringRecord::new();

        {
            let headers = rdr.headers().unwrap();
            assert_eq!(3, headers.len());
            assert_eq!("foo", &headers[0]);
            assert_eq!("bar", &headers[1]);
            assert_eq!("baz", &headers[2]);
        }

        assert!(rdr.read_record(&mut rec).unwrap());
        assert_eq!(3, rec.len());
        assert_eq!("foo", &rec[0]);

        assert!(rdr.read_record(&mut rec).unwrap());
        assert_eq!(3, rec.len());
        assert_eq!("a", &rec[0]);

        assert!(rdr.read_record(&mut rec).unwrap());
        assert_eq!(3, rec.len());
        assert_eq!("d", &rec[0]);

        assert!(!rdr.read_record(&mut rec).unwrap());
    }

    #[test]
    fn read_record_no_headers_after() {
        let data = b("foo,bar,baz\na,b,c\nd,e,f");
        let mut rdr =
            ReaderBuilder::new().has_headers(false).from_reader(data);
        let mut rec = StringRecord::new();

        assert!(rdr.read_record(&mut rec).unwrap());
        assert_eq!(3, rec.len());
        assert_eq!("foo", &rec[0]);

        assert!(rdr.read_record(&mut rec).unwrap());
        assert_eq!(3, rec.len());
        assert_eq!("a", &rec[0]);

        assert!(rdr.read_record(&mut rec).unwrap());
        assert_eq!(3, rec.len());
        assert_eq!("d", &rec[0]);

        assert!(!rdr.read_record(&mut rec).unwrap());

        let headers = rdr.headers().unwrap();
        assert_eq!(3, headers.len());
        assert_eq!("foo", &headers[0]);
        assert_eq!("bar", &headers[1]);
        assert_eq!("baz", &headers[2]);
    }

    #[test]
    fn seek() {
        let data = b("foo,bar,baz\na,b,c\nd,e,f\ng,h,i");
        let mut rdr = ReaderBuilder::new().from_reader(io::Cursor::new(data));
        rdr.seek(newpos(18, 3, 2)).unwrap();

        let mut rec = StringRecord::new();

        assert_eq!(18, rdr.position().byte());
        assert!(rdr.read_record(&mut rec).unwrap());
        assert_eq!(3, rec.len());
        assert_eq!("d", &rec[0]);

        assert_eq!(24, rdr.position().byte());
        assert_eq!(4, rdr.position().line());
        assert_eq!(3, rdr.position().record());
        assert!(rdr.read_record(&mut rec).unwrap());
        assert_eq!(3, rec.len());
        assert_eq!("g", &rec[0]);

        assert!(!rdr.read_record(&mut rec).unwrap());
    }

    // Test that we can read headers after seeking even if the headers weren't
    // explicit read before seeking.
    #[test]
    fn seek_headers_after() {
        let data = b("foo,bar,baz\na,b,c\nd,e,f\ng,h,i");
        let mut rdr = ReaderBuilder::new().from_reader(io::Cursor::new(data));
        rdr.seek(newpos(18, 3, 2)).unwrap();
        assert_eq!(rdr.headers().unwrap(), vec!["foo", "bar", "baz"]);
    }

    // Test that we can read headers after seeking if the headers were read
    // before seeking.
    #[test]
    fn seek_headers_before_after() {
        let data = b("foo,bar,baz\na,b,c\nd,e,f\ng,h,i");
        let mut rdr = ReaderBuilder::new().from_reader(io::Cursor::new(data));
        let headers = rdr.headers().unwrap().clone();
        rdr.seek(newpos(18, 3, 2)).unwrap();
        assert_eq!(&headers, rdr.headers().unwrap());
    }

    // Test that even if we didn't read headers before seeking, if we seek to
    // the current byte offset, then no seeking is done and therefore we can
    // still read headers after seeking.
    #[test]
    fn seek_headers_no_actual_seek() {
        let data = b("foo,bar,baz\na,b,c\nd,e,f\ng,h,i");
        let mut rdr = ReaderBuilder::new().from_reader(io::Cursor::new(data));
        rdr.seek(Position::new()).unwrap();
        assert_eq!("foo", &rdr.headers().unwrap()[0]);
    }

    // Test that position info is reported correctly in absence of headers.
    #[test]
    fn positions_no_headers() {
        let mut rdr = ReaderBuilder::new()
            .has_headers(false)
            .from_reader("a,b,c\nx,y,z".as_bytes())
            .into_records();

        let pos = rdr.next().unwrap().unwrap().position().unwrap().clone();
        assert_eq!(pos.byte(), 0);
        assert_eq!(pos.line(), 1);
        assert_eq!(pos.record(), 0);

        let pos = rdr.next().unwrap().unwrap().position().unwrap().clone();
        assert_eq!(pos.byte(), 6);
        assert_eq!(pos.line(), 2);
        assert_eq!(pos.record(), 1);
    }

    // Test that position info is reported correctly with headers.
    #[test]
    fn positions_headers() {
        let mut rdr = ReaderBuilder::new()
            .has_headers(true)
            .from_reader("a,b,c\nx,y,z".as_bytes())
            .into_records();

        let pos = rdr.next().unwrap().unwrap().position().unwrap().clone();
        assert_eq!(pos.byte(), 6);
        assert_eq!(pos.line(), 2);
        assert_eq!(pos.record(), 1);
    }

    // Test that reading headers on empty data yields an empty record.
    #[test]
    fn headers_on_empty_data() {
        let mut rdr = ReaderBuilder::new().from_reader("".as_bytes());
        let r = rdr.byte_headers().unwrap();
        assert_eq!(r.len(), 0);
    }

    // Test that reading the first record on empty data works.
    #[test]
    fn no_headers_on_empty_data() {
        let mut rdr =
            ReaderBuilder::new().has_headers(false).from_reader("".as_bytes());
        assert_eq!(rdr.records().count(), 0);
    }

    // Test that reading the first record on empty data works, even if
    // we've tried to read headers before hand.
    #[test]
    fn no_headers_on_empty_data_after_headers() {
        let mut rdr =
            ReaderBuilder::new().has_headers(false).from_reader("".as_bytes());
        assert_eq!(rdr.headers().unwrap().len(), 0);
        assert_eq!(rdr.records().count(), 0);
    }
}
