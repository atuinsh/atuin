use std::error::Error;
use std::io;
use std::process;

// This introduces a type alias so that we can conveniently reference our
// record type.
type Record = (String, String, Option<u64>, f64, f64);

fn run() -> Result<(), Box<dyn Error>> {
    let mut rdr = csv::Reader::from_reader(io::stdin());
    // Instead of creating an iterator with the `records` method, we create
    // an iterator with the `deserialize` method.
    for result in rdr.deserialize() {
        // We must tell Serde what type we want to deserialize into.
        let record: Record = result?;
        println!("{:?}", record);
    }
    Ok(())
}

fn main() {
    if let Err(err) = run() {
        println!("{}", err);
        process::exit(1);
    }
}
