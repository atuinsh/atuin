use std::error::Error;
use std::io;
use std::process;

fn main() {
    if let Err(err) = run() {
        println!("{}", err);
        process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let mut rdr = csv::Reader::from_reader(io::stdin());
    for result in rdr.records() {
        // This is effectively the same code as our `match` in the
        // previous example. In other words, `?` is syntactic sugar.
        let record = result?;
        println!("{:?}", record);
    }
    Ok(())
}
