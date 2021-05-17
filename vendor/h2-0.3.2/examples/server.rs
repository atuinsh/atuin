use std::error::Error;

use bytes::Bytes;
use h2::server;
use tokio::net::{TcpListener, TcpStream};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    let _ = env_logger::try_init();

    let listener = TcpListener::bind("127.0.0.1:5928").await?;

    println!("listening on {:?}", listener.local_addr());

    loop {
        if let Ok((socket, _peer_addr)) = listener.accept().await {
            tokio::spawn(async move {
                if let Err(e) = handle(socket).await {
                    println!("  -> err={:?}", e);
                }
            });
        }
    }
}

async fn handle(socket: TcpStream) -> Result<(), Box<dyn Error + Send + Sync>> {
    let mut connection = server::handshake(socket).await?;
    println!("H2 connection bound");

    while let Some(result) = connection.accept().await {
        let (request, mut respond) = result?;
        println!("GOT request: {:?}", request);
        let response = http::Response::new(());

        let mut send = respond.send_response(response, false)?;

        println!(">>>> sending data");
        send.send_data(Bytes::from_static(b"hello world"), true)?;
    }

    println!("~~~~~~~~~~~~~~~~~~~~~~~~~~~ H2 connection CLOSE !!!!!! ~~~~~~~~~~~");

    Ok(())
}
