use std::error::Error;
use std::io;
use std::process;

use serde::Serialize;

// Note that structs can derive both Serialize and Deserialize!
#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
struct Record<'a> {
    city: &'a str,
    state: &'a str,
    population: Option<u64>,
    latitude: f64,
    longitude: f64,
}

fn run() -> Result<(), Box<dyn Error>> {
    let mut wtr = csv::Writer::from_writer(io::stdout());

    wtr.serialize(Record {
        city: "Davidsons Landing",
        state: "AK",
        population: None,
        latitude: 65.2419444,
        longitude: -165.2716667,
    })?;
    wtr.serialize(Record {
        city: "Kenai",
        state: "AK",
        population: Some(7610),
        latitude: 60.5544444,
        longitude: -151.2583333,
    })?;
    wtr.serialize(Record {
        city: "Oakman",
        state: "AL",
        population: None,
        latitude: 33.7133333,
        longitude: -87.3886111,
    })?;

    wtr.flush()?;
    Ok(())
}

fn main() {
    if let Err(err) = run() {
        println!("{}", err);
        process::exit(1);
    }
}
