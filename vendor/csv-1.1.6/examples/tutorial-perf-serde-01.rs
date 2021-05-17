use std::error::Error;
use std::io;
use std::process;

use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Record {
    country: String,
    city: String,
    accent_city: String,
    region: String,
    population: Option<u64>,
    latitude: f64,
    longitude: f64,
}

fn run() -> Result<u64, Box<dyn Error>> {
    let mut rdr = csv::Reader::from_reader(io::stdin());

    let mut count = 0;
    for result in rdr.deserialize() {
        let record: Record = result?;
        if record.country == "us" && record.region == "MA" {
            count += 1;
        }
    }
    Ok(count)
}

fn main() {
    match run() {
        Ok(count) => {
            println!("{}", count);
        }
        Err(err) => {
            println!("{}", err);
            process::exit(1);
        }
    }
}
