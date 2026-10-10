use eyre::Result;

use crate::settings::Settings;

pub async fn logout(settings: &Settings) -> Result<()> {
    let meta = settings.meta_store().await?;

    if meta.logged_in().await? {
        meta.delete_session().await?;
        meta.delete_hub_session().await?;
        println!("You have logged out!");
    } else {
        println!("You are not logged in");
    }

    Ok(())
}
