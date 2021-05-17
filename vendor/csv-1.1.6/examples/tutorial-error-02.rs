use std::io;
use std::process;

fn main() {
    let mut rdr = csv::Reader::from_reader(io::stdin());
    for result in rdr.records() {
        // Examine our Result.
        // If there was no problem, print the record.
        // Otherwise, print the error message and quit the program.
        match result {
            Ok(record) => println!("{:?}", record),
            Err(err) => {
                println!("error reading CSV from <stdin>: {}", err);
                process::exit(1);
            }
        }
    }
}
