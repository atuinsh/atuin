use atuin_common::encryption::paseto_v4;
use atuin_domain::api::LoginRequest;
use eyre::Result;

use crate::api_client;
use crate::record::sqlite_store::SqliteStore;
use crate::settings::Settings;

pub async fn login(
    settings: &Settings,
    store: &SqliteStore,
    username: String,
    password: String,
    key: String,
) -> Result<String> {
    // try parse the key as a mnemonic...
    let key = paseto_v4::Key::try_from_mnemonic(&key)?;

    let key_path = &settings.key_path;

    if key_path.exists() {
        // we now know that the user has logged in specifying a key, AND that the key path
        // exists

        // 1. check if the saved key and the provided key match. if so, nothing to do.
        // 2. if not, re-encrypt the local history and overwrite the key
        let current_key = paseto_v4::Key::try_load_from_path(&settings.key_path)?;

        if key != current_key {
            println!("\nRe-encrypting local store with new key");

            store.re_encrypt(&current_key, &key).await?;

            println!("Writing new key");
            key.overwrite_path(key_path)?;
        }
    } else {
        key.try_write_path(key_path)?;
    }

    let session = api_client::login(
        &settings.sync_address,
        LoginRequest { username, password },
        &settings.extra_headers,
    )
    .await?;

    Settings::meta_store().await?.save_session(&session.session).await?;

    Ok(session.session)
}
