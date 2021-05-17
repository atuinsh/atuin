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
fn test_receive_after_init_close() {
    env_logger::init();

    spawn(|| {
        sleep(Duration::from_secs(5));
        println!("Unit test executed too long, perhaps stuck on WOULDBLOCK...");
        exit(1);
    });

    let server = TcpListener::bind("127.0.0.1:3013").unwrap();

    let client_thread = spawn(move || {
        let (mut client, _) = connect(Url::parse("ws://localhost:3013/socket").unwrap()).unwrap();

        client.write_message(Message::Text("Hello WebSocket".into())).unwrap();

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

    // This read should succeed even though we already initiated a close
    let message = client_handler.read_message().unwrap();
    assert_eq!(message.into_data(), b"Hello WebSocket");

    assert!(client_handler.read_message().unwrap().is_close()); // receive acknowledgement

    let err = client_handler.read_message().unwrap_err(); // now we should get ConnectionClosed
    match err {
        Error::ConnectionClosed => {}
        _ => panic!("unexpected error: {:?}", err),
    }

    drop(client_handler);

    client_thread.join().unwrap();
}
