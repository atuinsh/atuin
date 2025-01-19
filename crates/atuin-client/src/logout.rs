use eyre::{Context, Result};
use fs_err::remove_file;

use crate::settings::Settings;

pub fn logout(settings: &Settings) -> Result<()> {
    let session_path = settings.session_path.as_str();

    if settings.logged_in() {
        remove_file(session_path).context(t!("Failed to remove session file"))?;
        println!("{}", t!("You have logged out!"));
    } else {
        println!("{}", t!("You are not logged in"));
    }

    Ok(())
}
