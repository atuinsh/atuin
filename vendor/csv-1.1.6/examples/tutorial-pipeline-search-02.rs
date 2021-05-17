use std::env;
use std::error::Error;
use std::io;
use std::process;

fn run() -> Result<(), Box<dyn Error>> {
    let query = match env::args().nth(1) {
        None => return Err(From::from("expected 1 argument, but got none")),
        Some(query) => query,
    };

    let mut rdr = csv::Reader::from_reader(io::stdin());
    let mut wtr = csv::Writer::from_writer(io::stdout());

    wtr.write_record(rdr.byte_headers()?)?;

    for result in rdr.byte_records() {
        let record = result?;
        // `query` is a `String` while `field` is now a `&[u8]`, so we'll
        // need to convert `query` to `&[u8]` before doing a comparison.
        if record.iter().any(|field| field == query.as_bytes()) {
            wtr.write_record(&record)?;
        }
    }

    wtr.flush()?;
    Ok(())
}

fn main() {
    if let Err(err) = run() {
        println!("{}", err);
        process::exit(1);
    }
}
