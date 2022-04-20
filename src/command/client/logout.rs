use eyre::{Context, Result};
use fs_err::remove_file;

pub fn run() -> Result<()> {
    let session_path = atuin_common::utils::data_dir().join("session");

    if session_path.exists() {
        remove_file(session_path.as_path()).context("Failed to remove session file")?;
        println!("You have logged out!");
    } else {
        println!("You are not logged in");
    }

    Ok(())
}
