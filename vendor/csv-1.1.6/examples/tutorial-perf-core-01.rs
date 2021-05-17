use std::io::{self, Read};
use std::process;

use csv_core::{ReadFieldResult, Reader};

fn run(mut data: &[u8]) -> Option<u64> {
    let mut rdr = Reader::new();

    // Count the number of records in Massachusetts.
    let mut count = 0;
    // Indicates the current field index. Reset to 0 at start of each record.
    let mut fieldidx = 0;
    // True when the current record is in the United States.
    let mut inus = false;
    // Buffer for field data. Must be big enough to hold the largest field.
    let mut field = [0; 1024];
    loop {
        // Attempt to incrementally read the next CSV field.
        let (result, nread, nwrite) = rdr.read_field(data, &mut field);
        // nread is the number of bytes read from our input. We should never
        // pass those bytes to read_field again.
        data = &data[nread..];
        // nwrite is the number of bytes written to the output buffer `field`.
        // The contents of the buffer after this point is unspecified.
        let field = &field[..nwrite];

        match result {
            // We don't need to handle this case because we read all of the
            // data up front. If we were reading data incrementally, then this
            // would be a signal to read more.
            ReadFieldResult::InputEmpty => {}
            // If we get this case, then we found a field that contains more
            // than 1024 bytes. We keep this example simple and just fail.
            ReadFieldResult::OutputFull => {
                return None;
            }
            // This case happens when we've successfully read a field. If the
            // field is the last field in a record, then `record_end` is true.
            ReadFieldResult::Field { record_end } => {
                if fieldidx == 0 && field == b"us" {
                    inus = true;
                } else if inus && fieldidx == 3 && field == b"MA" {
                    count += 1;
                }
                if record_end {
                    fieldidx = 0;
                    inus = false;
                } else {
                    fieldidx += 1;
                }
            }
            // This case happens when the CSV reader has successfully exhausted
            // all input.
            ReadFieldResult::End => {
                break;
            }
        }
    }
    Some(count)
}

fn main() {
    // Read the entire contents of stdin up front.
    let mut data = vec![];
    if let Err(err) = io::stdin().read_to_end(&mut data) {
        println!("{}", err);
        process::exit(1);
    }
    match run(&data) {
        None => {
            println!("error: could not count records, buffer too small");
            process::exit(1);
        }
        Some(count) => {
            println!("{}", count);
        }
    }
}
