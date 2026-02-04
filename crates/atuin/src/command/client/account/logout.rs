use eyre::Result;

pub async fn run() -> Result<()> {
    atuin_client::logout::logout().await
}
