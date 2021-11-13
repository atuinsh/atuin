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
    pub username: Option<String>,

    #[structopt(long, short)]
    pub email: Option<String>,

    #[structopt(long, short)]
    pub password: Option<String>,
}

pub fn run(
    settings: &Settings,
    username: &Option<String>,
    email: &Option<String>,
    password: &Option<String>,
) -> Result<()> {
    use super::login::or_user_input;
    let username = or_user_input(username, "username");
    let email = or_user_input(email, "email");
    let password = or_user_input(password, "password");

    let session = api_client::register(
        settings.sync_address.as_str(),
        &username,
        &email,
        &password,
    )?;

    let path = settings.session_path.as_str();
    let mut file = File::create(path)?;
    file.write_all(session.session.as_bytes())?;

    // Create a new key, and save it to disk
    let _key = atuin_client::encryption::new_key(settings)?;

    Ok(())
}
