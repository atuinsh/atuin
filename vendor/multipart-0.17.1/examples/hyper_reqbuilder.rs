extern crate hyper;
extern crate multipart;

use hyper::Client;

use multipart::client::lazy::Multipart;

fn main() {
    let mut binary = "Hello world in binary!".as_bytes();

    let _response = Multipart::new()
        .add_text("text", "Hello, world!")
        .add_file("file", "lorem_ipsum.txt")
        // A little extra type info needed.
        .add_stream("binary", &mut binary, None as Option<&str>, None)
        // Request is sent here
        .client_request(&Client::new(), "http://localhost:80")
        .expect("Error sending multipart request");
}