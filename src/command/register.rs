use std::collections::HashMap;
use std::fs::File;
use std::io::prelude::*;

use eyre::{eyre, Result};
use structopt::StructOpt;

use crate::settings::Settings;

#[derive(StructOpt)]
#[structopt(setting(structopt::clap::AppSettings::DeriveDisplayOrder))]
pub struct Cmd {
    #[structopt(long, short)]
    pub username: String,

    #[structopt(long, short)]
    pub email: String,

    #[structopt(long, short)]
    pub password: String,
}

pub fn run(settings: &Settings, username: String, email: String, password: String) -> Result<()> {
    let mut map = HashMap::new();
    map.insert("username", username.as_str());
    map.insert("email", email.as_str());
    map.insert("password", password.as_str());

    let url = format!("{}/user/{}", settings.local.sync_address, username);
    let resp = reqwest::blocking::get(url)?;

    if resp.status().is_success() {
        println!("Username is already in use! Please try another.");
        return Ok(());
    }

    let url = format!("{}/register", settings.local.sync_address);
    let client = reqwest::blocking::Client::new();
    let resp = client.post(url).json(&map).send()?;

    if !resp.status().is_success() {
        println!("Failed to register user - please check your details and try again");
        return Err(eyre!("failed to register user"));
    }

    let session = resp.json::<HashMap<String, String>>()?;
    let session = session["session"].clone();

    let path = settings.local.session_path.as_str();
    let mut file = File::create(path)?;
    file.write_all(session.as_bytes())?;

    Ok(())
}
