#![deny(warnings)]

use tokio::net::UnixListener;
use tokio_stream::wrappers::UnixListenerStream;

#[tokio::main]
async fn main() {
    pretty_env_logger::init();

    let listener = UnixListener::bind("/tmp/warp.sock").unwrap();
    let incoming = UnixListenerStream::new(listener);
    warp::serve(warp::fs::dir("examples/dir"))
        .run_incoming(incoming)
        .await;
}
