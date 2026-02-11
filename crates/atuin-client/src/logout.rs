use eyre::Result;

use crate::settings::Settings;

pub async fn logout() -> Result<()> {
    let meta = Settings::meta_store().await?;

    if meta.logged_in().await? {
        meta.delete_session().await?;
        println!("You have logged out!");
    } else {
        println!("You are not logged in");
    }

    Ok(())
}
