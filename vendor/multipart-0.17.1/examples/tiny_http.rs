extern crate tiny_http;
extern crate multipart;

use std::io::{self, Cursor, Write};
use multipart::server::{Multipart, Entries, SaveResult};
use multipart::mock::StdoutTee;
use tiny_http::{Response, StatusCode, Request};
fn main() {
    // Starting a server on `localhost:80`
    let server = tiny_http::Server::http("localhost:80").expect("Could not bind localhost:80");
    loop {
        // This blocks until the next request is received
        let mut request = server.recv().unwrap();

        // Processes a request and returns response or an occured error
        let result = process_request(&mut request);
        let resp = match result {
            Ok(resp) => resp,
            Err(e) => {
                println!("An error has occured during request proccessing: {:?}", e);
                build_response(500, "The received data was not correctly proccessed on the server")
            }
        };

        // Answers with a response to a client
        request.respond(resp).unwrap();
    }
}

type RespBody = Cursor<Vec<u8>>;

/// Processes a request and returns response or an occured error.
fn process_request(request: &mut Request) -> io::Result<Response<RespBody>> {
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
                    // We don't set limits
                    Err(reason.unwrap_err())
                }
                SaveResult::Error(error) => Err(error),
            }
        }
        Err(_) => Ok(build_response(400, "The request is not multipart")),
    }
}

/// Processes saved entries from multipart request.
/// Returns an OK response or an error.
fn process_entries(entries: Entries) -> io::Result<Response<RespBody>> {
    let mut data = Vec::new();

    {
        let stdout = io::stdout();
        let tee = StdoutTee::new(&mut data, &stdout);
        entries.write_debug(tee)?;
    }

    writeln!(data, "Entries processed")?;

    Ok(build_response(200, data))
}

fn build_response<D: Into<Vec<u8>>>(status_code: u16, data: D) -> Response<RespBody> {
    let data = data.into();
    let data_len = data.len();
    Response::new(StatusCode(status_code),
                  vec![],
                  Cursor::new(data),
                  Some(data_len),
                  None)
}
