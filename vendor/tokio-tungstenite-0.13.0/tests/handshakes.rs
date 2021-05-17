use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::{accept_async, client_async};

#[tokio::test]
async fn handshakes() {
    let (tx, rx) = futures_channel::oneshot::channel();

    let f = async move {
        let listener = TcpListener::bind("0.0.0.0:12345").await.unwrap();
        tx.send(()).unwrap();
        while let Ok((connection, _)) = listener.accept().await {
            let stream = accept_async(connection).await;
            stream.expect("Failed to handshake with connection");
        }
    };

    tokio::spawn(f);

    rx.await.expect("Failed to wait for server to be ready");
    let tcp = TcpStream::connect("0.0.0.0:12345").await.expect("Failed to connect");
    let url = url::Url::parse("ws://localhost:12345/").unwrap();
    let _stream = client_async(url, tcp).await.expect("Client failed to connect");
}
