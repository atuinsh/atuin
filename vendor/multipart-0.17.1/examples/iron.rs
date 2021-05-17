extern crate multipart;
extern crate iron;

extern crate env_logger;

use std::io::{self, Write};
use multipart::mock::StdoutTee;
use multipart::server::{Multipart, Entries, SaveResult};
use iron::prelude::*;
use iron::status;

fn main() {
    env_logger::init();

    Iron::new(process_request).http("localhost:80").expect("Could not bind localhost:80");
}

/// Processes a request and returns response or an occured error.
fn process_request(request: &mut Request) -> IronResult<Response> {
    // Getting a multipart reader wrapper
    match Multipart::from_request(request) {
        Ok(mut multipart) => {
            // Fetching all data and processing it.
            // save().temp() reads the request fully, parsing all fields and saving all files
            // in a new temporary directory under the OS temporary directory.
            match multipart.save().temp() {
                SaveResult::Full(entries) => process_entries(entries),
                SaveResult::Partial(entries, reason) => {
                    process_entries(entries.keep_partial())?;
                    Ok(Response::with((
                        status::BadRequest,
                        format!("error reading request: {}", reason.unwrap_err())
                    )))
                }
                SaveResult::Error(error) => Ok(Response::with((
                    status::BadRequest,
                    format!("error reading request: {}", error)
                ))),
            }
        }
        Err(_) => {
            Ok(Response::with((status::BadRequest, "The request is not multipart")))
        }
    }
}

/// Processes saved entries from multipart request.
/// Returns an OK response or an error.
fn process_entries(entries: Entries) -> IronResult<Response> {
    let mut data = Vec::new();

    {
        let stdout = io::stdout();
        let tee = StdoutTee::new(&mut data, &stdout);
        entries.write_debug(tee).map_err(|e| {
            IronError::new(
                e,
                (status::InternalServerError, "Error printing request fields")
            )
        })?;
    }

    let _ = writeln!(data, "Entries processed");

    Ok(Response::with((status::Ok, data)))
}
