/*!
`csv-core` provides a fast CSV reader and writer for use in a `no_std` context.

This crate will never use the standard library. `no_std` support is therefore
enabled by default.

If you're looking for more ergonomic CSV parsing routines, please use the
[`csv`](https://docs.rs/csv) crate.

# Overview

This crate has two primary APIs. The `Reader` API provides a CSV parser, and
the `Writer` API provides a CSV writer.

# Example: reading CSV

This example shows how to count the number of fields and records in CSV data.

```
use csv_core::{Reader, ReadFieldResult};

let data = "
foo,bar,baz
a,b,c
xxx,yyy,zzz
";

let mut rdr = Reader::new();
let mut bytes = data.as_bytes();
let mut count_fields = 0;
let mut count_records = 0;
loop {
    // We skip handling the output since we don't need it for counting.
    let (result, nin, _) = rdr.read_field(bytes, &mut [0; 1024]);
    bytes = &bytes[nin..];
    match result {
        ReadFieldResult::InputEmpty => {},
        ReadFieldResult::OutputFull => panic!("field too large"),
        ReadFieldResult::Field { record_end } => {
            count_fields += 1;
            if record_end {
                count_records += 1;
            }
        }
        ReadFieldResult::End => break,
    }
}
assert_eq!(3, count_records);
assert_eq!(9, count_fields);
```

# Example: writing CSV

This example shows how to use the `Writer` API to write valid CSV data. Proper
quoting is handled automatically.

```
use csv_core::Writer;

// This is where we'll write out CSV data.
let mut out = &mut [0; 1024];
// The number of bytes we've written to `out`.
let mut nout = 0;
// Create a CSV writer with a default configuration.
let mut wtr = Writer::new();

// Write a single field. Note that we ignore the `WriteResult` and the number
// of input bytes consumed since we're doing this by hand.
let (_, _, n) = wtr.field(&b"foo"[..], &mut out[nout..]);
nout += n;

// Write a delimiter and then another field that requires quotes.
let (_, n) = wtr.delimiter(&mut out[nout..]);
nout += n;
let (_, _, n) = wtr.field(&b"bar,baz"[..], &mut out[nout..]);
nout += n;
let (_, n) = wtr.terminator(&mut out[nout..]);
nout += n;

// Now write another record.
let (_, _, n) = wtr.field(&b"a \"b\" c"[..], &mut out[nout..]);
nout += n;
let (_, n) = wtr.delimiter(&mut out[nout..]);
nout += n;
let (_, _, n) = wtr.field(&b"quux"[..], &mut out[nout..]);
nout += n;

// We must always call finish once done writing.
// This ensures that any closing quotes are written.
let (_, n) = wtr.finish(&mut out[nout..]);
nout += n;

assert_eq!(&out[..nout], &b"\
foo,\"bar,baz\"
\"a \"\"b\"\" c\",quux"[..]);
```
*/

#![deny(missing_docs)]
#![no_std]

pub use crate::reader::{
    ReadFieldNoCopyResult, ReadFieldResult, ReadRecordNoCopyResult,
    ReadRecordResult, Reader, ReaderBuilder,
};
pub use crate::writer::{
    is_non_numeric, quote, WriteResult, Writer, WriterBuilder,
};

mod reader;
mod writer;

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
    /// Checks whether the terminator is set to CRLF.
    fn is_crlf(&self) -> bool {
        match *self {
            Terminator::CRLF => true,
            Terminator::Any(_) => false,
            _ => unreachable!(),
        }
    }

    fn equals(&self, other: u8) -> bool {
        match *self {
            Terminator::CRLF => other == b'\r' || other == b'\n',
            Terminator::Any(b) => other == b,
            _ => unreachable!(),
        }
    }
}

impl Default for Terminator {
    fn default() -> Terminator {
        Terminator::CRLF
    }
}

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

impl Default for QuoteStyle {
    fn default() -> QuoteStyle {
        QuoteStyle::Necessary
    }
}
