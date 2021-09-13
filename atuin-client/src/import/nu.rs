// import old shell history!
// automatically hoover up all that we can find

use std::{
    ops::Range,
    path::{Path, PathBuf},
};

use directories::ProjectDirs;
use eyre::{eyre, Result};

use super::Importer;
use crate::history::History;

use rustyline::history;

pub struct Nu {
    hist: history::History,
    iter: Range<usize>,
}

impl Importer for Nu {
    const NAME: &'static str = "nu";

    fn histpath() -> Result<PathBuf> {
        // https://github.com/nushell/nushell/blob/55eafadf025fb8737ed922b2f471137e0cdfe1f9/crates/nu-data/src/config.rs#L197-L199
        let dir = ProjectDirs::from("org", "nushell", "nu")
            .ok_or_else(|| eyre!("could not find nushell history path"))?;
        let path = ProjectDirs::data_local_dir(&dir).to_owned();

        Ok(path)
    }

    fn parse(path: &impl AsRef<Path>) -> Result<Self> {
        let mut hist = history::History::new();
        hist.load(path)?;
        let len = hist.len();
        Ok(Self {
            hist,
            iter: 0..len,
        })
    }
}

impl Iterator for Nu {
    type Item = Result<History>;

    fn next(&mut self) -> Option<Self::Item> {
        let index = self.iter.next()?;
        let item = self.hist.get(index)?;

        let time = chrono::Utc::now();
        let offset = chrono::Duration::seconds(index as i64);
        let time = time - offset;

        Some(Ok(History::new(
            time,
            item.trim_end().to_string(),
            String::from("unknown"),
            -1,
            -1,
            None,
            None,
        )))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.iter.size_hint()
    }
}
