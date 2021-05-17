//! Verifies that the server returns a `ConnectionClosed` error when the connection
//! is closedd from the server's point of view and drop the underlying tcp socket.

use std::{
    net::{TcpListener, TcpStream},
    process::exit,
    thread::{sleep, spawn},
    time::Duration,
};

use native_tls::TlsStream;
use net2::TcpStreamExt;
use tungstenite::{accept, connect, stream::Stream, Error, Message, WebSocket};
use url::Url;

type Sock = WebSocket<Stream<TcpStream, TlsStream<TcpStream>>>;

fn do_test<CT, ST>(port: u16, client_task: CT, server_task: ST)
where
    CT: FnOnce(Sock) + Send + 'static,
    ST: FnOnce(WebSocket<TcpStream>),
{
    env_logger::try_init().ok();

    spawn(|| {
        sleep(Duration::from_secs(5));
        println!("Unit test executed too long, perhaps stuck on WOULDBLOCK...");
        exit(1);
    });

    let server =
        TcpListener::bind(("127.0.0.1", port)).expect("Can't listen, is port already in use?");

    let client_thread = spawn(move || {
        let (client, _) = connect(Url::parse(&format!("ws://localhost:{}/socket", port)).unwrap())
            .expect("Can't connect to port");

        client_task(client);
    });

    let client_handler = server.incoming().next().unwrap();
    let client_handler = accept(client_handler.unwrap()).unwrap();

    server_task(client_handler);

    client_thread.join().unwrap();
}

#[test]
fn test_server_close() {
    do_test(
        3012,
        |mut cli_sock| {
            cli_sock.write_message(Message::Text("Hello WebSocket".into())).unwrap();

            let message = cli_sock.read_message().unwrap(); // receive close from server
            assert!(message.is_close());

            let err = cli_sock.read_message().unwrap_err(); // now we should get ConnectionClosed
            match err {
                Error::ConnectionClosed => {}
                _ => panic!("unexpected error: {:?}", err),
            }
        },
        |mut srv_sock| {
            let message = srv_sock.read_message().unwrap();
            assert_eq!(message.into_data(), b"Hello WebSocket");

            srv_sock.close(None).unwrap(); // send close to client

            let message = srv_sock.read_message().unwrap(); // receive acknowledgement
            assert!(message.is_close());

            let err = srv_sock.read_message().unwrap_err(); // now we should get ConnectionClosed
            match err {
                Error::ConnectionClosed => {}
                _ => panic!("unexpected error: {:?}", err),
            }
        },
    );
}

#[test]
fn test_evil_server_close() {
    do_test(
        3013,
        |mut cli_sock| {
            cli_sock.write_message(Message::Text("Hello WebSocket".into())).unwrap();

            sleep(Duration::from_secs(1));

            let message = cli_sock.read_message().unwrap(); // receive close from server
            assert!(message.is_close());

            let err = cli_sock.read_message().unwrap_err(); // now we should get ConnectionClosed
            match err {
                Error::ConnectionClosed => {}
                _ => panic!("unexpected error: {:?}", err),
            }
        },
        |mut srv_sock| {
            let message = srv_sock.read_message().unwrap();
            assert_eq!(message.into_data(), b"Hello WebSocket");

            srv_sock.close(None).unwrap(); // send close to client

            let message = srv_sock.read_message().unwrap(); // receive acknowledgement
            assert!(message.is_close());
            // and now just drop the connection without waiting for `ConnectionClosed`
            srv_sock.get_mut().set_linger(Some(Duration::from_secs(0))).unwrap();
            drop(srv_sock);
        },
    );
}

#[test]
fn test_client_close() {
    do_test(
        3014,
        |mut cli_sock| {
            cli_sock.write_message(Message::Text("Hello WebSocket".into())).unwrap();

            let message = cli_sock.read_message().unwrap(); // receive answer from server
            assert_eq!(message.into_data(), b"From Server");

            cli_sock.close(None).unwrap(); // send close to server

            let message = cli_sock.read_message().unwrap(); // receive acknowledgement from server
            assert!(message.is_close());

            let err = cli_sock.read_message().unwrap_err(); // now we should get ConnectionClosed
            match err {
                Error::ConnectionClosed => {}
                _ => panic!("unexpected error: {:?}", err),
            }
        },
        |mut srv_sock| {
            let message = srv_sock.read_message().unwrap();
            assert_eq!(message.into_data(), b"Hello WebSocket");

            srv_sock.write_message(Message::Text("From Server".into())).unwrap();

            let message = srv_sock.read_message().unwrap(); // receive close from client
            assert!(message.is_close());

            let err = srv_sock.read_message().unwrap_err(); // now we should get ConnectionClosed
            match err {
                Error::ConnectionClosed => {}
                _ => panic!("unexpected error: {:?}", err),
            }
        },
    );
}
