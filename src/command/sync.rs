use eyre::Result;

use crate::local::database::Database;
use crate::local::sync;
use crate::settings::Settings;

pub fn run(settings: &Settings, force: bool, db: &mut impl Database) -> Result<()> {
    sync::sync(settings, force, db)?;
    println!(
        "Sync complete! {} items in database, force: {}",
        db.history_count()?,
        force
    );
    Ok(())
}
