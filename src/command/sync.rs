use eyre::Result;

use crate::local::database::Database;
use crate::local::sync;
use crate::settings::Settings;

pub async fn run(settings: &Settings, force: bool, db: &mut (impl Database + Send)) -> Result<()> {
    sync::sync(settings, force, db).await?;
    println!(
        "Sync complete! {} items in database, force: {}",
        db.history_count()?,
        force
    );
    Ok(())
}
