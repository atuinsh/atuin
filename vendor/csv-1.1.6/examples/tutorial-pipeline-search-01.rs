use std::env;
use std::error::Error;
use std::io;
use std::process;

fn run() -> Result<(), Box<dyn Error>> {
    // Get the query from the positional arguments.
    // If one doesn't exist, return an error.
    let query = match env::args().nth(1) {
        None => return Err(From::from("expected 1 argument, but got none")),
        Some(query) => query,
    };

    // Build CSV readers and writers to stdin and stdout, respectively.
    let mut rdr = csv::Reader::from_reader(io::stdin());
    let mut wtr = csv::Writer::from_writer(io::stdout());

    // Before reading our data records, we should write the header record.
    wtr.write_record(rdr.headers()?)?;

    // Iterate over all the records in `rdr`, and write only records containing
    // `query` to `wtr`.
    for result in rdr.records() {
        let record = result?;
        if record.iter().any(|field| field == &query) {
            wtr.write_record(&record)?;
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
