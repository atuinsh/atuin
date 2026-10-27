use atuin_common::userio;
use eyre::Result;

use crate::settings::Settings;

pub async fn logout(settings: &Settings) -> Result<()> {
    let meta = settings.meta_store().await?;

    if meta.logged_in().await? {
        meta.delete_session().await?;
        meta.delete_hub_session().await?;
        userio::log::info!("You have logged out!");
    } else {
        userio::log::info!("You are not logged in");
    }

    Ok(())
}
