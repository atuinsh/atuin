csv
===
A fast and flexible CSV reader and writer for Rust, with support for Serde.

[![Linux build status](https://api.travis-ci.org/BurntSushi/rust-csv.svg)](https://travis-ci.org/BurntSushi/rust-csv)
[![Windows build status](https://ci.appveyor.com/api/projects/status/github/BurntSushi/rust-csv?svg=true)](https://ci.appveyor.com/project/BurntSushi/rust-csv)
[![](http://meritbadge.herokuapp.com/csv)](https://crates.io/crates/csv)

Dual-licensed under MIT or the [UNLICENSE](http://unlicense.org).


### Documentation

https://docs.rs/csv

If you're new to Rust, the
[tutorial](https://docs.rs/csv/1.0.0/csv/tutorial/index.html)
is a good place to start.


### Usage

Add this to your `Cargo.toml`:

```toml
[dependencies]
csv = "1.1"
```

### Example

This example shows how to read CSV data from stdin and print each record to
stdout.

There are more examples in the
[cookbook](https://docs.rs/csv/1.0.0/csv/cookbook/index.html).

```rust
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

```text
$ git clone git://github.com/BurntSushi/rust-csv
$ cd rust-csv
$ cargo run --example cookbook-read-basic < examples/data/smallpop.csv
```

### Example with Serde

This example shows how to read CSV data from stdin into your own custom struct.
By default, the member names of the struct are matched with the values in the
header record of your CSV data.

```rust
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

```text
$ git clone git://github.com/BurntSushi/rust-csv
$ cd rust-csv
$ cargo run --example cookbook-read-serde < examples/data/smallpop.csv
```
