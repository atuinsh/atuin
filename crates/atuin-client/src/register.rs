use eyre::Result;

use crate::{api_client, settings::Settings};

pub async fn register_classic(
    settings: &Settings,
    username: String,
    email: String,
    password: String,
) -> Result<String> {
    let session =
        api_client::register(settings.sync_address.as_str(), &username, &email, &password).await?;

    let meta = Settings::meta_store().await?;
    meta.save_session(&session.session).await?;

    let _key = crate::encryption::load_key(settings)?;

    Ok(session.session)
}
