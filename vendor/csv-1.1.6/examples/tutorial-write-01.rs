use std::error::Error;
use std::io;
use std::process;

fn run() -> Result<(), Box<dyn Error>> {
    let mut wtr = csv::Writer::from_writer(io::stdout());
    // Since we're writing records manually, we must explicitly write our
    // header record. A header record is written the same way that other
    // records are written.
    wtr.write_record(&[
        "City",
        "State",
        "Population",
        "Latitude",
        "Longitude",
    ])?;
    wtr.write_record(&[
        "Davidsons Landing",
        "AK",
        "",
        "65.2419444",
        "-165.2716667",
    ])?;
    wtr.write_record(&["Kenai", "AK", "7610", "60.5544444", "-151.2583333"])?;
    wtr.write_record(&["Oakman", "AL", "", "33.7133333", "-87.3886111"])?;

    // A CSV writer maintains an internal buffer, so it's important
    // to flush the buffer when you're done.
    wtr.flush()?;
    Ok(())
}

fn main() {
    if let Err(err) = run() {
        println!("{}", err);
        process::exit(1);
    }
}
