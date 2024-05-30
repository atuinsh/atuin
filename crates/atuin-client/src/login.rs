use std::path::PathBuf;

use atuin_common::api::LoginRequest;
use eyre::{bail, Context, Result};
use tokio::fs::File;
use tokio::io::AsyncWriteExt;

use crate::{
    api_client,
    encryption::{decode_key, encode_key, load_key, Key},
    record::{sqlite_store::SqliteStore, store::Store},
    settings::Settings,
};

pub async fn login(
    settings: &Settings,
    store: &SqliteStore,
    username: String,
    password: String,
    key: String,
) -> Result<String> {
    // try parse the key as a mnemonic...
    let key = match bip39::Mnemonic::from_phrase(&key, bip39::Language::English) {
        Ok(mnemonic) => encode_key(Key::from_slice(mnemonic.entropy()))?,
        Err(err) => {
            if let Some(err) = err.downcast_ref::<bip39::ErrorKind>() {
                match err {
                    // assume they copied in the base64 key
                    bip39::ErrorKind::InvalidWord => key,
                    bip39::ErrorKind::InvalidChecksum => {
                        bail!("key mnemonic was not valid")
                    }
                    bip39::ErrorKind::InvalidKeysize(_)
                    | bip39::ErrorKind::InvalidWordLength(_)
                    | bip39::ErrorKind::InvalidEntropyLength(_, _) => {
                        bail!("key was not the correct length")
                    }
                }
            } else {
                // unknown error. assume they copied the base64 key
                key
            }
        }
    };

    let key_path = settings.key_path.as_str();
    let key_path = PathBuf::from(key_path);

    if !key_path.exists() {
        if decode_key(key.clone()).is_err() {
            bail!("the specified key was invalid");
        }

        let mut file = File::create(key_path).await?;
        file.write_all(key.as_bytes()).await?;
    } else {
        // we now know that the user has logged in specifying a key, AND that the key path
        // exists

        // 1. check if the saved key and the provided key match. if so, nothing to do.
        // 2. if not, re-encrypt the local history and overwrite the key
        let current_key: [u8; 32] = load_key(settings)?.into();

        let encoded = key.clone(); // gonna want to save it in a bit
        let new_key: [u8; 32] = decode_key(key)
            .context("could not decode provided key - is not valid base64")?
            .into();

        if new_key != current_key {
            println!("\nRe-encrypting local store with new key");

            store.re_encrypt(&current_key, &new_key).await?;

            println!("Writing new key");
            let mut file = File::create(key_path).await?;
            file.write_all(encoded.as_bytes()).await?;
        }
    }

    let session = api_client::login(
        settings.sync_address.as_str(),
        LoginRequest { username, password },
    )
    .await?;

    let session_path = settings.session_path.as_str();
    let mut file = File::create(session_path).await?;
    file.write_all(session.session.as_bytes()).await?;

    Ok(session.session)
}
