use std::error::Error;
use std::io;
use std::process;

fn run() -> Result<(), Box<dyn Error>> {
    let mut rdr = csv::Reader::from_reader(io::stdin());
    {
        // We nest this call in its own scope because of lifetimes.
        let headers = rdr.headers()?;
        println!("{:?}", headers);
    }
    for result in rdr.records() {
        let record = result?;
        println!("{:?}", record);
    }
    // We can ask for the headers at any time. There's no need to nest this
    // call in its own scope because we never try to borrow the reader again.
    let headers = rdr.headers()?;
    println!("{:?}", headers);
    Ok(())
}

fn main() {
    if let Err(err) = run() {
        println!("{}", err);
        process::exit(1);
    }
}
