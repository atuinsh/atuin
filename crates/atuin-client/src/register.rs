use eyre::Result;
use tokio::fs::File;
use tokio::io::AsyncWriteExt;

use crate::{api_client, settings::Settings};

pub async fn register(
    settings: &Settings,
    username: String,
    email: String,
    password: String,
) -> Result<String> {
    let session =
        api_client::register(settings.sync_address.as_str(), &username, &email, &password).await?;

    let path = settings.session_path.as_str();
    let mut file = File::create(path).await?;
    file.write_all(session.session.as_bytes()).await?;

    let _key = crate::encryption::load_key(settings)?;

    Ok(session.session)
}
