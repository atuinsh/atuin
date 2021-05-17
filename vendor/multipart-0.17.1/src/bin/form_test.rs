extern crate hyper;
extern crate multipart;

use multipart::server::Multipart;

use hyper::header::ContentType;
use hyper::server::*;

use std::fs::File;
use std::io;

fn main() {
    let listening = Server::http("127.0.0.1:0").expect("failed to bind socket")
        .handle(read_multipart).expect("failed to handle request");

    println!("bound socket to: {}", listening.socket);
}

fn read_multipart(req: Request, mut resp: Response) {
    if let Ok(mut multipart) = Multipart::from_request(req) {
        if let Err(e) = multipart.foreach_entry(|_| {}) {
            println!("error handling field: {}", e);
        }
    }

    let mut file = File::open("src/bin/test_form.html")
        .expect("failed to open src/bind/test_form.html");

    resp.headers_mut().set(ContentType("text/html".parse().unwrap()));

    let mut resp = resp.start().expect("failed to open response");
    io::copy(&mut file, &mut resp).expect("failed to write response");
}
