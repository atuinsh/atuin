use std::error::Error;
use std::io;
use std::process;

fn run() -> Result<(), Box<dyn Error>> {
    let mut wtr = csv::Writer::from_writer(io::stdout());

    // We still need to write headers manually.
    wtr.write_record(&[
        "City",
        "State",
        "Population",
        "Latitude",
        "Longitude",
    ])?;

    // But now we can write records by providing a normal Rust value.
    //
    // Note that the odd `None::<u64>` syntax is required because `None` on
    // its own doesn't have a concrete type, but Serde needs a concrete type
    // in order to serialize it. That is, `None` has type `Option<T>` but
    // `None::<u64>` has type `Option<u64>`.
    wtr.serialize((
        "Davidsons Landing",
        "AK",
        None::<u64>,
        65.2419444,
        -165.2716667,
    ))?;
    wtr.serialize(("Kenai", "AK", Some(7610), 60.5544444, -151.2583333))?;
    wtr.serialize(("Oakman", "AL", None::<u64>, 33.7133333, -87.3886111))?;

    wtr.flush()?;
    Ok(())
}

fn main() {
    if let Err(err) = run() {
        println!("{}", err);
        process::exit(1);
    }
}
