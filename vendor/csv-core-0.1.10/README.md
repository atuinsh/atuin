csv-core
========
A fast CSV reader and write for use in a `no_std` context. This crate will
never use the Rust standard library.

[![Linux build status](https://api.travis-ci.org/BurntSushi/rust-csv.png)](https://travis-ci.org/BurntSushi/rust-csv)
[![Windows build status](https://ci.appveyor.com/api/projects/status/github/BurntSushi/rust-csv?svg=true)](https://ci.appveyor.com/project/BurntSushi/rust-csv)
[![](http://meritbadge.herokuapp.com/csv-core)](https://crates.io/crates/csv-core)

Dual-licensed under MIT or the [UNLICENSE](http://unlicense.org).

### Documentation

https://docs.rs/csv-core

### Usage

Add this to your `Cargo.toml`:

```toml
[dependencies]
csv-core = "0.1.6"
```

### Build features

This crate by default links with `libc`, which is done via the `libc` feature.
Disabling this feature will drop `csv-core`'s dependency on `libc`.


### Example: reading CSV

This example shows how to count the number of fields and records in CSV data.

```rust
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


### Example: writing CSV

This example shows how to use the `Writer` API to write valid CSV data. Proper
quoting is handled automatically.

```rust
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
