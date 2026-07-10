// import old shell history!
// automatically hoover up all that we can find

use std::path::PathBuf;

use async_trait::async_trait;
use directories::BaseDirs;
use eyre::{Result, eyre};
use time::{Duration, OffsetDateTime};

use super::{Importer, Loader, unix_byte_lines};
use crate::history::History;
use crate::import::read_to_end;

#[derive(Debug)]
pub struct Nu {
    bytes: Vec<u8>,
}

impl Nu {
    fn num_entries(&self) -> usize {
        super::count_lines(&self.bytes)
    }
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
        let bytes = read_to_end(get_histpath()?)?;
        Ok(Self { bytes })
    }

    async fn entries(&mut self) -> Result<usize> {
        Ok(self.num_entries())
    }

    async fn load(self, h: &mut impl Loader) -> Result<()> {
        // Separate commands by 1ms as with bash and zsh
        let timestamp_increment = Duration::milliseconds(1);

        // Subtract enough milliseconds so the most recent command's timestamp will be now.
        let mut timestamp = OffsetDateTime::now_utc()
            - timestamp_increment
                * u32::try_from(self.num_entries().saturating_sub(1)).unwrap_or(u32::MAX);

        for b in unix_byte_lines(&self.bytes) {
            let current_timestamp = timestamp;
            timestamp += timestamp_increment;

            let s = match std::str::from_utf8(b) {
                Ok(s) => s,
                Err(_) => continue, // we can skip past things like invalid utf8
            };

            let cmd: String = s.replace("<\\n>", "\n");

            let entry = History::import().timestamp(current_timestamp).command(cmd);

            h.push(entry.build().into()).await?;
        }

        Ok(())
    }
}
