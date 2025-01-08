// import old shell history from atuin-histdb file!
// automatically hoover up all that we can find

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use eyre::{eyre, Result};

use crate::database::{Sqlite, Database, current_context};
use crate::settings::FilterMode;

use super::Importer;
use crate::history::History;
use crate::import::Loader;

pub struct AtuinExternalDb {
    histdb: Vec<History>,
}
impl AtuinExternalDb {
    pub fn histpath() -> Result<PathBuf> {
        let path = Path::new("/tmp/history.db").to_path_buf();
        if path.exists() {
            Ok(path)
        } else {
            Err(eyre!("Could not find history file hardcodded at /tmp/history.db"))
        }
    }
}

/// Read db at given file, return vector of entries.
async fn hist_from_db(dbpath: PathBuf) -> Result<Vec<History>> {
    let db_to_migrate = Sqlite::new(dbpath, 0.1).await?;
    let external_history = db_to_migrate.list(
        &[FilterMode::Global],
        &current_context(), // this is passed to list, but only used for the workspace
        None,
        false,
        true,
    ).await.unwrap();
    // debug!("{}", external_history);
    Ok(external_history)
}

#[async_trait]
impl Importer for AtuinExternalDb {
    const NAME: &'static str = "history.db";

    /// Creates a new AtuinExternalDb imports the history
    /// Does no duplicate checking.
    async fn new() -> Result<Self> {
        let dbpath = AtuinExternalDb::histpath()?;
        let histdb_entry_vec = hist_from_db(dbpath).await?;
        Ok(Self {
            histdb: histdb_entry_vec,
        })
    }

    async fn entries(&mut self) -> Result<usize> {
        Ok(self.histdb.len())
    }

    async fn load(self, h: &mut impl Loader) -> Result<()> {
        for entry in self.histdb {
            h.push(entry).await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod test {
    // TODO
}
