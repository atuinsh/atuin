/*!
The `csv` crate provides a fast and flexible CSV reader and writer, with
support for Serde.

The [tutorial](tutorial/index.html) is a good place to start if you're new to
Rust.

The [cookbook](cookbook/index.html) will give you a variety of complete Rust
programs that do CSV reading and writing.

# Brief overview

**If you're new to Rust**, you might find the
[tutorial](tutorial/index.html)
to be a good place to start.

The primary types in this crate are
[`Reader`](struct.Reader.html)
and
[`Writer`](struct.Writer.html),
for reading and writing CSV data respectively.
Correspondingly, to support CSV data with custom field or record delimiters
(among many other things), you should use either a
[`ReaderBuilder`](struct.ReaderBuilder.html)
or a
[`WriterBuilder`](struct.WriterBuilder.html),
depending on whether you're reading or writing CSV data.

Unless you're using Serde, the standard CSV record types are
[`StringRecord`](struct.StringRecord.html)
and
[`ByteRecord`](struct.ByteRecord.html).
`StringRecord` should be used when you know your data to be valid UTF-8.
For data that may be invalid UTF-8, `ByteRecord` is suitable.

Finally, the set of errors is described by the
[`Error`](struct.Error.html)
type.

The rest of the types in this crate mostly correspond to more detailed errors,
position information, configuration knobs or iterator types.

# Setup

Add this to your `Cargo.toml`:

```toml
[dependencies]
csv = "1.1"
```

If you want to use Serde's custom derive functionality on your custom structs,
then add this to your `[dependencies]` section of `Cargo.toml`:

```toml
[dependencies]
serde = { version = "1", features = ["derive"] }
```

# Example

This example shows how to read CSV data from stdin and print each record to
stdout.

There are more examples in the [cookbook](cookbook/index.html).

```no_run
use std::error::Error;
use std::io;
use std::process;

fn example() -> Result<(), Box<dyn Error>> {
    // Build the CSV reader and iterate over each record.
    let mut rdr = csv::Reader::from_reader(io::stdin());
    for result in rdr.records() {
        // The iterator yields Result<StringRecord, Error>, so we check the
        // error here.
        let record = result?;
        println!("{:?}", record);
    }
    Ok(())
}

fn main() {
    if let Err(err) = example() {
        println!("error running example: {}", err);
        process::exit(1);
    }
}
```

The above example can be run like so:

```ignore
$ git clone git://github.com/BurntSushi/rust-csv
$ cd rust-csv
$ cargo run --example cookbook-read-basic < examples/data/smallpop.csv
```

# Example with Serde

This example shows how to read CSV data from stdin into your own custom struct.
By default, the member names of the struct are matched with the values in the
header record of your CSV data.

```no_run
use std::error::Error;
use std::io;
use std::process;

use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct Record {
    city: String,
    region: String,
    country: String,
    population: Option<u64>,
}

fn example() -> Result<(), Box<dyn Error>> {
    let mut rdr = csv::Reader::from_reader(io::stdin());
    for result in rdr.deserialize() {
        // Notice that we need to provide a type hint for automatic
        // deserialization.
        let record: Record = result?;
        println!("{:?}", record);
    }
    Ok(())
}

fn main() {
    if let Err(err) = example() {
        println!("error running example: {}", err);
        process::exit(1);
    }
}
```

The above example can be run like so:

```ignore
$ git clone git://github.com/BurntSushi/rust-csv
$ cd rust-csv
$ cargo run --example cookbook-read-serde < examples/data/smallpop.csv
```

*/

#![deny(missing_docs)]

use std::result;

use serde::{Deserialize, Deserializer};

pub use crate::byte_record::{ByteRecord, ByteRecordIter, Position};
pub use crate::deserializer::{DeserializeError, DeserializeErrorKind};
pub use crate::error::{
    Error, ErrorKind, FromUtf8Error, IntoInnerError, Result, Utf8Error,
};
pub use crate::reader::{
    ByteRecordsIntoIter, ByteRecordsIter, DeserializeRecordsIntoIter,
    DeserializeRecordsIter, Reader, ReaderBuilder, StringRecordsIntoIter,
    StringRecordsIter,
};
pub use crate::string_record::{StringRecord, StringRecordIter};
pub use crate::writer::{Writer, WriterBuilder};

mod byte_record;
pub mod cookbook;
mod deserializer;
mod error;
mod reader;
mod serializer;
mod string_record;
pub mod tutorial;
mod writer;

/// The quoting style to use when writing CSV data.
#[derive(Clone, Copy, Debug)]
pub enum QuoteStyle {
    /// This puts quotes around every field. Always.
    Always,
    /// This puts quotes around fields only when necessary.
    ///
    /// They are necessary when fields contain a quote, delimiter or record
    /// terminator. Quotes are also necessary when writing an empty record
    /// (which is indistinguishable from a record with one empty field).
    ///
    /// This is the default.
    Necessary,
    /// This puts quotes around all fields that are non-numeric. Namely, when
    /// writing a field that does not parse as a valid float or integer, then
    /// quotes will be used even if they aren't strictly necessary.
    NonNumeric,
    /// This *never* writes quotes, even if it would produce invalid CSV data.
    Never,
    /// Hints that destructuring should not be exhaustive.
    ///
    /// This enum may grow additional variants, so this makes sure clients
    /// don't count on exhaustive matching. (Otherwise, adding a new variant
    /// could break existing code.)
    #[doc(hidden)]
    __Nonexhaustive,
}

impl QuoteStyle {
    fn to_core(self) -> csv_core::QuoteStyle {
        match self {
            QuoteStyle::Always => csv_core::QuoteStyle::Always,
            QuoteStyle::Necessary => csv_core::QuoteStyle::Necessary,
            QuoteStyle::NonNumeric => csv_core::QuoteStyle::NonNumeric,
            QuoteStyle::Never => csv_core::QuoteStyle::Never,
            _ => unreachable!(),
        }
    }
}

impl Default for QuoteStyle {
    fn default() -> QuoteStyle {
        QuoteStyle::Necessary
    }
}

/// A record terminator.
///
/// Use this to specify the record terminator while parsing CSV. The default is
/// CRLF, which treats `\r`, `\n` or `\r\n` as a single record terminator.
#[derive(Clone, Copy, Debug)]
pub enum Terminator {
    /// Parses `\r`, `\n` or `\r\n` as a single record terminator.
    CRLF,
    /// Parses the byte given as a record terminator.
    Any(u8),
    /// Hints that destructuring should not be exhaustive.
    ///
    /// This enum may grow additional variants, so this makes sure clients
    /// don't count on exhaustive matching. (Otherwise, adding a new variant
    /// could break existing code.)
    #[doc(hidden)]
    __Nonexhaustive,
}

impl Terminator {
    /// Convert this to the csv_core type of the same name.
    fn to_core(self) -> csv_core::Terminator {
        match self {
            Terminator::CRLF => csv_core::Terminator::CRLF,
            Terminator::Any(b) => csv_core::Terminator::Any(b),
            _ => unreachable!(),
        }
    }
}

impl Default for Terminator {
    fn default() -> Terminator {
        Terminator::CRLF
    }
}

/// The whitespace preservation behaviour when reading CSV data.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Trim {
    /// Preserves fields and headers. This is the default.
    None,
    /// Trim whitespace from headers.
    Headers,
    /// Trim whitespace from fields, but not headers.
    Fields,
    /// Trim whitespace from fields and headers.
    All,
    /// Hints that destructuring should not be exhaustive.
    ///
    /// This enum may grow additional variants, so this makes sure clients
    /// don't count on exhaustive matching. (Otherwise, adding a new variant
    /// could break existing code.)
    #[doc(hidden)]
    __Nonexhaustive,
}

impl Trim {
    fn should_trim_fields(&self) -> bool {
        self == &Trim::Fields || self == &Trim::All
    }

    fn should_trim_headers(&self) -> bool {
        self == &Trim::Headers || self == &Trim::All
    }
}

impl Default for Trim {
    fn default() -> Trim {
        Trim::None
    }
}

/// A custom Serde deserializer for possibly invalid `Option<T>` fields.
///
/// When deserializing CSV data, it is sometimes desirable to simply ignore
/// fields with invalid data. For example, there might be a field that is
/// usually a number, but will occasionally contain garbage data that causes
/// number parsing to fail.
///
/// You might be inclined to use, say, `Option<i32>` for fields such at this.
/// By default, however, `Option<i32>` will either capture *empty* fields with
/// `None` or valid numeric fields with `Some(the_number)`. If the field is
/// non-empty and not a valid number, then deserialization will return an error
/// instead of using `None`.
///
/// This function allows you to override this default behavior. Namely, if
/// `Option<T>` is deserialized with non-empty but invalid data, then the value
/// will be `None` and the error will be ignored.
///
/// # Example
///
/// This example shows how to parse CSV records with numerical data, even if
/// some numerical data is absent or invalid. Without the
/// `serde(deserialize_with = "...")` annotations, this example would return
/// an error.
///
/// ```
/// use std::error::Error;
///
/// use csv::Reader;
/// use serde::Deserialize;
///
/// #[derive(Debug, Deserialize, Eq, PartialEq)]
/// struct Row {
///     #[serde(deserialize_with = "csv::invalid_option")]
///     a: Option<i32>,
///     #[serde(deserialize_with = "csv::invalid_option")]
///     b: Option<i32>,
///     #[serde(deserialize_with = "csv::invalid_option")]
///     c: Option<i32>,
/// }
///
/// # fn main() { example().unwrap(); }
/// fn example() -> Result<(), Box<dyn Error>> {
///     let data = "\
/// a,b,c
/// 5,\"\",xyz
/// ";
///     let mut rdr = Reader::from_reader(data.as_bytes());
///     if let Some(result) = rdr.deserialize().next() {
///         let record: Row = result?;
///         assert_eq!(record, Row { a: Some(5), b: None, c: None });
///         Ok(())
///     } else {
///         Err(From::from("expected at least one record but got none"))
///     }
/// }
/// ```
pub fn invalid_option<'de, D, T>(de: D) -> result::Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    Option<T>: Deserialize<'de>,
{
    Option::<T>::deserialize(de).or_else(|_| Ok(None))
}
