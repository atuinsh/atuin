use std::collections::HashMap;
use std::fs::File;
use std::io::prelude::*;

use eyre::{eyre, Result};
use structopt::StructOpt;

use atuin_client::settings::Settings;

#[derive(StructOpt)]
#[structopt(setting(structopt::clap::AppSettings::DeriveDisplayOrder))]
pub struct Cmd {
    #[structopt(long, short)]
    pub username: String,

    #[structopt(long, short)]
    pub password: String,

    #[structopt(long, short, about = "the encryption key for your account")]
    pub key: String,
}

impl Cmd {
    pub fn run(&self, settings: &Settings) -> Result<()> {
        let mut map = HashMap::new();
        map.insert("username", self.username.clone());
        map.insert("password", self.password.clone());

        let url = format!("{}/login", settings.sync_address);
        let client = reqwest::blocking::Client::new();

        let resp = client.post(url).json(&map).send()?;

        if resp.status() != reqwest::StatusCode::OK {
            return Err(eyre!("invalid login details"));
        }

        let session = resp.json::<HashMap<String, String>>()?;
        let session = session["session"].clone();

        let session_path = settings.session_path.as_str();
        let mut file = File::create(session_path)?;
        file.write_all(session.as_bytes())?;

        let key_path = settings.key_path.as_str();
        let mut file = File::create(key_path)?;
        file.write_all(&base64::decode(self.key.clone())?)?;

        println!("Logged in!");

        Ok(())
    }
}
