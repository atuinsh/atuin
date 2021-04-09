use std::collections::HashMap;
use std::fs::File;
use std::io::prelude::*;

use eyre::Result;
use structopt::StructOpt;

use crate::settings::Settings;

#[derive(StructOpt)]
#[structopt(setting(structopt::clap::AppSettings::DeriveDisplayOrder))]
pub struct Cmd {
    #[structopt(long, short)]
    pub username: String,

    #[structopt(long, short)]
    pub password: String,
}

pub fn run(settings: &Settings, username: String, password: String) -> Result<()> {
    let mut map = HashMap::new();
    map.insert("username", username);
    map.insert("password", password);

    let url = format!("{}/login", settings.local.sync_address);
    let client = reqwest::blocking::Client::new();
    let resp = client.post(url).json(&map).send()?;

    let session = resp.json::<HashMap<String, String>>()?;
    let session = session["session"].clone();

    let path = settings.local.session_path.as_str();
    let mut file = File::create(path)?;
    file.write_all(session.as_bytes())?;

    println!("Logged in!");

    Ok(())
}
