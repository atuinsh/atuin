pub mod commands;

#[tokio::main]
async fn main() -> eyre::Result<()> {
    commands::run().await
}
