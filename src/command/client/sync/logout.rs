use std::path::PathBuf;

use eyre::{Context, Result};
use fs_err::remove_file;

use atuin_client::settings::Settings;

pub fn run(settings: &Settings) -> Result<()> {
    let session_path = settings.session_path.as_str();

    if PathBuf::from(session_path).exists() {
        remove_file(session_path).context("Failed to remove session file")?;
        println!("You have logged out!");
    } else {
        println!("You are not logged in");
    }

    Ok(())
}
