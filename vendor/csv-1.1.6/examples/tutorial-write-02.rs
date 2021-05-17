use std::env;
use std::error::Error;
use std::ffi::OsString;
use std::process;

fn run() -> Result<(), Box<dyn Error>> {
    let file_path = get_first_arg()?;
    let mut wtr = csv::Writer::from_path(file_path)?;

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

    wtr.flush()?;
    Ok(())
}

/// Returns the first positional argument sent to this process. If there are no
/// positional arguments, then this returns an error.
fn get_first_arg() -> Result<OsString, Box<dyn Error>> {
    match env::args_os().nth(1) {
        None => Err(From::from("expected 1 argument, but got none")),
        Some(file_path) => Ok(file_path),
    }
}

fn main() {
    if let Err(err) = run() {
        println!("{}", err);
        process::exit(1);
    }
}
