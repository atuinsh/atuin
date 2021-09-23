use std::fs::File;
use std::io::prelude::*;
use std::io;

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

fn get_input() -> Result<String> {
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    Ok(input.trim_end_matches(&['\r', '\n'][..]).to_string())
}

pub fn run(settings: &Settings, username: Option<String>, email: Option<String>, password: Option<String>) -> Result<()> {
    let username = if let Some(username) = username { username } else {
        eprint!("Please enter username: ");
        get_input().expect("Failed to read username from input")
    };

    let email = if let Some(email) = email { email } else {
        eprint!("Please enter email: ");
        get_input().expect("Failed to read email from input")
    };

    let password = if let Some(password) = password { password } else {
        eprint!("Please enter password: ");
        get_input().expect("Failed to read password from input")
    };

    let session = api_client::register(settings.sync_address.as_str(), username.as_str(), email.as_str(), password.as_str())?;

    let path = settings.session_path.as_str();
    let mut file = File::create(path)?;
    file.write_all(session.session.as_bytes())?;

    // Create a new key, and save it to disk
    let _key = atuin_client::encryption::new_key(settings)?;

    Ok(())
}
