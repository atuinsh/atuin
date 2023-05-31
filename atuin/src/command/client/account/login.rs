use std::{io, path::PathBuf};

use clap::Parser;
use eyre::{bail, Context, Result};
use tokio::{fs::File, io::AsyncWriteExt};

use atuin_client::{
    api_client,
    encryption::{decode_key, encode_key, new_key, Key},
    settings::Settings,
};
use atuin_common::api::LoginRequest;
use rpassword::prompt_password;

#[derive(Parser)]
pub struct Cmd {
    #[clap(long, short)]
    pub username: Option<String>,

    #[clap(long, short)]
    pub password: Option<String>,

    /// The encryption key for your account
    #[clap(long, short)]
    pub key: Option<String>,
}

fn get_input() -> Result<String> {
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    Ok(input.trim_end_matches(&['\r', '\n'][..]).to_string())
}

impl Cmd {
    pub async fn run(&self, settings: &Settings) -> Result<()> {
        let session_path = settings.session_path.as_str();

        if PathBuf::from(session_path).exists() {
            println!(
                "You are already logged in! Please run 'atuin logout' if you wish to login again"
            );

            return Ok(());
        }

        let username = or_user_input(&self.username, "username");
        let key = or_user_input(&self.key, "encryption key [blank to use existing key file]");
        let password = self.password.clone().unwrap_or_else(read_user_password);

        let key_path = settings.key_path.as_str();
        if key.is_empty() {
            if PathBuf::from(key_path).exists() {
                let bytes = fs_err::read_to_string(key_path)
                    .context("existing key file couldn't be read")?;
                if decode_key(bytes).is_err() {
                    bail!("the key in existing key file was invalid");
                }
            } else {
                println!("No key file exists, creating a new");
                let _key = new_key(settings)?;
            }
        } else {
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

            if decode_key(key.clone()).is_err() {
                bail!("the specified key was invalid");
            }

            let mut file = File::create(key_path).await?;
            file.write_all(key.as_bytes()).await?;
        }

        let session = api_client::login(
            settings.sync_address.as_str(),
            LoginRequest { username, password },
        )
        .await?;

        let session_path = settings.session_path.as_str();
        let mut file = File::create(session_path).await?;
        file.write_all(session.session.as_bytes()).await?;

        println!("Logged in!");

        Ok(())
    }
}

pub(super) fn or_user_input(value: &'_ Option<String>, name: &'static str) -> String {
    value.clone().unwrap_or_else(|| read_user_input(name))
}

pub(super) fn read_user_password() -> String {
    let password = prompt_password("Please enter password: ");
    password.expect("Failed to read from input")
}

fn read_user_input(name: &'static str) -> String {
    eprint!("Please enter {name}: ");
    get_input().expect("Failed to read from input")
}

#[cfg(test)]
mod tests {
    use atuin_client::encryption::Key;

    #[test]
    fn mnemonic_round_trip() {
        let key = Key::from([
            3, 1, 4, 1, 5, 9, 2, 6, 5, 3, 5, 8, 9, 7, 9, 3, 2, 3, 8, 4, 6, 2, 6, 4, 3, 3, 8, 3, 2,
            7, 9, 5,
        ]);
        let phrase = bip39::Mnemonic::from_entropy(&key, bip39::Language::English)
            .unwrap()
            .into_phrase();
        let mnemonic = bip39::Mnemonic::from_phrase(&phrase, bip39::Language::English).unwrap();
        assert_eq!(mnemonic.entropy(), key.as_slice());
        assert_eq!(phrase, "adapt amused able anxiety mother adapt beef gaze amount else seat alcohol cage lottery avoid scare alcohol cactus school avoid coral adjust catch pink");
    }
}
