pub mod client;

#[tokio::main]
async fn main() -> eyre::Result<()> {
    client::run().await
}
