use eyre::Result;

use atuin_client::database::Database;
use atuin_client::settings::Settings;
use atuin_client::sync;

pub async fn run(settings: &Settings, force: bool, db: &mut (impl Database + Send)) -> Result<()> {
    sync::sync(settings, force, db).await?;
    println!(
        "Sync complete! {} items in database, force: {}",
        db.history_count()?,
        force
    );
    Ok(())
}
