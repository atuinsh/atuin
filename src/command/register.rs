use std::fs::File;
use std::io::prelude::*;

use eyre::Result;
use structopt::StructOpt;

use atuin_client::api_client;
use atuin_client::settings::Settings;

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

pub fn run(settings: &Settings, username: &str, email: &str, password: &str) -> Result<()> {
    let session = api_client::register(settings.sync_address.as_str(), username, email, password)?;

    let path = settings.session_path.as_str();
    let mut file = File::create(path)?;
    file.write_all(session.session.as_bytes())?;

    Ok(())
}
