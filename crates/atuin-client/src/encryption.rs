use std::io::prelude::*;

pub use atuin_common::encryption::paseto_v4;
use eyre::{Result, bail};
use fs_err as fs;

use crate::settings::Settings;

pub fn new_key(settings: &Settings) -> Result<paseto_v4::Key> {
    let path = &settings.key_path;

    if path.exists() {
        bail!("key already exists! cannot overwrite");
    }

    let key = paseto_v4::Key::generate();
    let encoded = key.encode();

    let mut file = fs::File::create(path)?;
    file.write_all(encoded.dangerously_leak_secret().as_bytes())?;

    Ok(key)
}

// Loads the secret key, will create + save if it doesn't exist
pub fn load_key(settings: &Settings) -> Result<paseto_v4::Key> {
    let path = &settings.key_path;

    let key = if path.exists() {
        let key = fs_err::read_to_string(path)?;
        paseto_v4::Key::decode(&key)?
    } else {
        new_key(settings)?
    };

    Ok(key)
}
