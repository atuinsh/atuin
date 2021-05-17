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
