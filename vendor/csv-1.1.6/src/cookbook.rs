/*!
A cookbook of examples for CSV reading and writing.

# List of examples

This is a list of examples that follow. Each of them can be found in the
`examples` directory of the
[`rust-csv`](https://github.com/BurntSushi/rust-csv)
repository.

For **reading** CSV:

1. [Basic](#reading-basic)
2. [With Serde](#reading-with-serde)
3. [Setting a different delimiter](#reading-setting-a-different-delimiter)
4. [Without headers](#reading-without-headers)

For **writing** CSV:

5. [Basic](#writing-basic)
6. [With Serde](#writing-with-serde)

Please
[submit a pull request](https://github.com/BurntSushi/rust-csv/pulls)
if you're interested in adding an example to this list!

# Reading: basic

This example shows how to read CSV data from stdin and print each record to
stdout.

```no_run
# //cookbook-read-basic.rs
use std::error::Error;
use std::io;
use std::process;

fn example() -> Result<(), Box<dyn Error>> {
    // Build the CSV reader and iterate over each record.
    let mut rdr = csv::Reader::from_reader(io::stdin());
    for result in rdr.records() {
        // The iterator yields Result<StringRecord, Error>, so we check the
        // error here..
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

# Reading: with Serde

This is like the previous example, except it shows how to deserialize each
record into a struct type that you define.

For more examples and details on how Serde deserialization works, see the
[`Reader::deserialize`](../struct.Reader.html#method.deserialize)
method.

```no_run
# //cookbook-read-serde.rs
use std::error::Error;
use std::io;
use std::process;

use serde::Deserialize;

// By default, struct field names are deserialized based on the position of
// a corresponding field in the CSV data's header record.
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

# Reading: setting a different delimiter

This example shows how to read CSV data from stdin where fields are separated
by `:` instead of `,`.

```no_run
# //cookbook-read-colon.rs
use std::error::Error;
use std::io;
use std::process;

fn example() -> Result<(), Box<dyn Error>> {
    let mut rdr = csv::ReaderBuilder::new()
        .delimiter(b':')
        .from_reader(io::stdin());
    for result in rdr.records() {
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
$ cargo run --example cookbook-read-colon < examples/data/smallpop-colon.csv
```

# Reading: without headers

The CSV reader in this crate assumes that CSV data has a header record by
default, but the setting can be toggled. When enabled, the first record in
CSV data in interpreted as the header record and is skipped. When disabled, the
first record is not skipped. This example shows how to disable that setting.

```no_run
# //cookbook-read-no-headers.rs
use std::error::Error;
use std::io;
use std::process;

fn example() -> Result<(), Box<dyn Error>> {
    let mut rdr = csv::ReaderBuilder::new()
        .has_headers(false)
        .from_reader(io::stdin());
    for result in rdr.records() {
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
$ cargo run --example cookbook-read-no-headers < examples/data/smallpop-no-headers.csv
```

# Writing: basic

This example shows how to write CSV data to stdout.

```no_run
# //cookbook-write-basic.rs
use std::error::Error;
use std::io;
use std::process;

fn example() -> Result<(), Box<dyn Error>> {
    let mut wtr = csv::Writer::from_writer(io::stdout());

    // When writing records without Serde, the header record is written just
    // like any other record.
    wtr.write_record(&["city", "region", "country", "population"])?;
    wtr.write_record(&["Southborough", "MA", "United States", "9686"])?;
    wtr.write_record(&["Northbridge", "MA", "United States", "14061"])?;
    wtr.flush()?;
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
$ cargo run --example cookbook-write-basic > /tmp/simplepop.csv
```

# Writing: with Serde

This example shows how to write CSV data to stdout with Serde. Namely, we
represent each record using a custom struct that we define. In this example,
headers are written automatically.

```no_run
# //cookbook-write-serde.rs
use std::error::Error;
use std::io;
use std::process;

use serde::Serialize;

#[derive(Debug, Serialize)]
struct Record {
    city: String,
    region: String,
    country: String,
    population: Option<u64>,
}

fn example() -> Result<(), Box<dyn Error>> {
    let mut wtr = csv::Writer::from_writer(io::stdout());

    // When writing records with Serde using structs, the header row is written
    // automatically.
    wtr.serialize(Record {
        city: "Southborough".to_string(),
        region: "MA".to_string(),
        country: "United States".to_string(),
        population: Some(9686),
    })?;
    wtr.serialize(Record {
        city: "Northbridge".to_string(),
        region: "MA".to_string(),
        country: "United States".to_string(),
        population: Some(14061),
    })?;
    wtr.flush()?;
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
$ cargo run --example cookbook-write-serde > /tmp/simplepop.csv
```
*/
