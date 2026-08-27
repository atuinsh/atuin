use atuin_common::encryption::paseto_v4;
use eyre::Result;

use crate::api_client;
use crate::settings::Settings;

pub async fn register_classic(
    settings: &Settings,
    username: String,
    email: String,
    password: String,
) -> Result<String> {
    let session = api_client::register(
        &settings.sync_address,
        &username,
        &email,
        &password,
        &settings.extra_headers,
    )
    .await?;

    let meta = Settings::meta_store().await?;
    meta.save_session(&session.session).await?;

    let _key = paseto_v4::Key::try_load_or_generate(&settings.key_path)?;

    Ok(session.session)
}
