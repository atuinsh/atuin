use std::path::PathBuf;

use crypto::digest::Digest;
use crypto::sha2::Sha256;
use sodiumoxide::crypto::pwhash::argon2id13;
use uuid::Uuid;

pub fn hash_secret(secret: &str) -> String {
    sodiumoxide::init().unwrap();
    let hash = argon2id13::pwhash(
        secret.as_bytes(),
        argon2id13::OPSLIMIT_INTERACTIVE,
        argon2id13::MEMLIMIT_INTERACTIVE,
    )
    .unwrap();
    let texthash = std::str::from_utf8(&hash.0).unwrap().to_string();

    // postgres hates null chars. don't do that to postgres
    texthash.trim_end_matches('\u{0}').to_string()
}

pub fn hash_str(string: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.input_str(string);

    hasher.result_str()
}

pub fn uuid_v4() -> String {
    Uuid::new_v4().to_simple().to_string()
}

pub fn config_dir() -> PathBuf {
    // TODO: more reliable, more tested
    // I don't want to use ProjectDirs, it puts config in awkward places on
    // mac. Data too. Seems to be more intended for GUI apps.
    let home = std::env::var("HOME").expect("$HOME not found");
    let home = PathBuf::from(home);

    let config_base = std::env::var("XDG_CONFIG_HOME").map_or_else(
        |_| {
            let mut config = home.clone();
            config.push(".config");
            config.push("atuin");
            config
        },
        PathBuf::from,
    );

    config_base
}

pub fn data_dir() -> PathBuf {
    // TODO: more reliable, more tested
    // I don't want to use ProjectDirs, it puts config in awkward places on
    // mac. Data too. Seems to be more intended for GUI apps.
    let home = std::env::var("HOME").expect("$HOME not found");
    let home = PathBuf::from(home);

    let data_base = std::env::var("XDG_DATA_HOME").map_or_else(
        |_| {
            let mut data = home.clone();
            data.push(".local");
            data.push("share");
            data.push("atuin");
            data
        },
        PathBuf::from,
    );

    data_base
}
