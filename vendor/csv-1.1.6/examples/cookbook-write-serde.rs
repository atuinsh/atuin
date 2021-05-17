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
