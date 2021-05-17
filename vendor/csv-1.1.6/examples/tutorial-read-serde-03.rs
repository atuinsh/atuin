use std::collections::HashMap;
use std::error::Error;
use std::io;
use std::process;

// This introduces a type alias so that we can conveniently reference our
// record type.
type Record = HashMap<String, String>;

fn run() -> Result<(), Box<dyn Error>> {
    let mut rdr = csv::Reader::from_reader(io::stdin());
    for result in rdr.deserialize() {
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
