use eyre::Result;

use crate::local::database::Database;
use crate::local::sync;
use crate::settings::Settings;

pub fn run(settings: &Settings, db: &mut impl Database) -> Result<()> {
    sync::sync(settings, db)?;
    println!("Sync complete! {} items in database", db.history_count()?);
    Ok(())
}
