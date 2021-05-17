use std::env;
use std::error::Error;
use std::io;
use std::process;

use serde::{Deserialize, Serialize};

// Unlike previous examples, we derive both Deserialize and Serialize. This
// means we'll be able to automatically deserialize and serialize this type.
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase")]
struct Record {
    city: String,
    state: String,
    population: Option<u64>,
    latitude: f64,
    longitude: f64,
}

fn run() -> Result<(), Box<dyn Error>> {
    // Get the query from the positional arguments.
    // If one doesn't exist or isn't an integer, return an error.
    let minimum_pop: u64 = match env::args().nth(1) {
        None => return Err(From::from("expected 1 argument, but got none")),
        Some(arg) => arg.parse()?,
    };

    // Build CSV readers and writers to stdin and stdout, respectively.
    // Note that we don't need to write headers explicitly. Since we're
    // serializing a custom struct, that's done for us automatically.
    let mut rdr = csv::Reader::from_reader(io::stdin());
    let mut wtr = csv::Writer::from_writer(io::stdout());

    // Iterate over all the records in `rdr`, and write only records containing
    // a population that is greater than or equal to `minimum_pop`.
    for result in rdr.deserialize() {
        // Remember that when deserializing, we must use a type hint to
        // indicate which type we want to deserialize our record into.
        let record: Record = result?;

        // `map_or` is a combinator on `Option`. It take two parameters:
        // a value to use when the `Option` is `None` (i.e., the record has
        // no population count) and a closure that returns another value of
        // the same type when the `Option` is `Some`. In this case, we test it
        // against our minimum population count that we got from the command
        // line.
        if record.population.map_or(false, |pop| pop >= minimum_pop) {
            wtr.serialize(record)?;
        }
    }

    // CSV writers use an internal buffer, so we should always flush when done.
    wtr.flush()?;
    Ok(())
}

fn main() {
    if let Err(err) = run() {
        println!("{}", err);
        process::exit(1);
    }
}
