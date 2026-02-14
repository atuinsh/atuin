pub mod commands;
pub mod tui;

#[tokio::main]
async fn main() -> eyre::Result<()> {
    commands::run().await
}
