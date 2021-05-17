//! Verifies that we can read data messages even if we have initiated a close handshake,
//! but before we got confirmation.

use std::{
    net::TcpListener,
    process::exit,
    thread::{sleep, spawn},
    time::Duration,
};

use tungstenite::{accept, connect, Error, Message};
use url::Url;

#[test]
fn test_no_send_after_close() {
    env_logger::init();

    spawn(|| {
        sleep(Duration::from_secs(5));
        println!("Unit test executed too long, perhaps stuck on WOULDBLOCK...");
        exit(1);
    });

    let server = TcpListener::bind("127.0.0.1:3013").unwrap();

    let client_thread = spawn(move || {
        let (mut client, _) = connect(Url::parse("ws://localhost:3013/socket").unwrap()).unwrap();

        let message = client.read_message().unwrap(); // receive close from server
        assert!(message.is_close());

        let err = client.read_message().unwrap_err(); // now we should get ConnectionClosed
        match err {
            Error::ConnectionClosed => {}
            _ => panic!("unexpected error: {:?}", err),
        }
    });

    let client_handler = server.incoming().next().unwrap();
    let mut client_handler = accept(client_handler.unwrap()).unwrap();

    client_handler.close(None).unwrap(); // send close to client

    let err = client_handler.write_message(Message::Text("Hello WebSocket".into()));

    assert!(err.is_err());

    match err.unwrap_err() {
        Error::Protocol(s) => assert_eq!("Sending after closing is not allowed", s),
        e => panic!("unexpected error: {:?}", e),
    }

    drop(client_handler);

    client_thread.join().unwrap();
}
