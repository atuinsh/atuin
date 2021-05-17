extern crate hyper;
extern crate multipart;

use hyper::client::Request;
use hyper::method::Method;
use hyper::net::Streaming;

use multipart::client::Multipart;

use std::io::Read;

fn main() {
    let url = "http://localhost:80".parse()
        .expect("Failed to parse URL");

    let request = Request::new(Method::Post, url)
        .expect("Failed to create request");

    let mut multipart = Multipart::from_request(request)
        .expect("Failed to create Multipart");

    write_body(&mut multipart)
        .expect("Failed to write multipart body");

    let mut response = multipart.send().expect("Failed to send multipart request");

    if !response.status.is_success() {
        let mut res = String::new();
        response.read_to_string(&mut res).expect("failed to read response");
        println!("response reported unsuccessful: {:?}\n {}", response, res);
    }

    // Optional: read out response
}

fn write_body(multi: &mut Multipart<Request<Streaming>>) -> hyper::Result<()> {
    let mut binary = "Hello world from binary!".as_bytes();

    multi.write_text("text", "Hello, world!")?;
    multi.write_file("file", "lorem_ipsum.txt")?;
    // &[u8] impl Read
    multi.write_stream("binary", &mut binary, None, None)
        .and(Ok(()))
}
