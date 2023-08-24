// import old shell history!
// automatically hoover up all that we can find

use std::{fs::File, io::Read, path::PathBuf};

use async_trait::async_trait;
use directories::BaseDirs;
use eyre::{eyre, Result};

use super::{unix_byte_lines, Importer, Loader};
use crate::history::History;

#[derive(Debug)]
pub struct Nu {
    bytes: Vec<u8>,
}

fn get_histpath() -> Result<PathBuf> {
    let base = BaseDirs::new().ok_or_else(|| eyre!("could not determine data directory"))?;
    let config_dir = base.config_dir().join("nushell");

    let histpath = config_dir.join("history.txt");
    if histpath.exists() {
        Ok(histpath)
    } else {
        Err(eyre!("Could not find history file."))
    }
}

#[async_trait]
impl Importer for Nu {
    const NAME: &'static str = "nu";

    async fn new() -> Result<Self> {
        let mut bytes = Vec::new();
        let path = get_histpath()?;
        let mut f = File::open(path)?;
        f.read_to_end(&mut bytes)?;
        Ok(Self { bytes })
    }

    async fn entries(&mut self) -> Result<usize> {
        Ok(super::count_lines(&self.bytes))
    }

    async fn load(self, h: &mut impl Loader) -> Result<()> {
        let now = chrono::Utc::now();

        let mut counter = 0;
        for b in unix_byte_lines(&self.bytes) {
            let s = match std::str::from_utf8(b) {
                Ok(s) => s,
                Err(_) => continue, // we can skip past things like invalid utf8
            };

            let cmd: String = s.replace("<\\n>", "\n");

            let offset = chrono::Duration::nanoseconds(counter);
            counter += 1;

            let entry = History::import().timestamp(now - offset).command(cmd);

            h.push(entry.build().into()).await?;
        }

        Ok(())
    }
}
