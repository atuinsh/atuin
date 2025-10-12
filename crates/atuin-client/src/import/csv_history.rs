use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;

use async_trait::async_trait;
use csv::Reader;
use eyre::{Result, eyre};
use time::OffsetDateTime;

use super::{Importer, Loader};
use crate::history::History;

const HISTORY_FILE: &str = "history.csv"; // Global constant for history file

#[derive(Debug)]
pub struct Csv;

fn default_histpath() -> Result<PathBuf> {
    let histpath = PathBuf::from(HISTORY_FILE);
    if histpath.exists() {
        Ok(histpath)
    } else {
        Err(eyre!(
            "Could not find history file. Please create 'history.csv' in the current directory."
        ))
    }
}

#[async_trait]
impl Importer for Csv {
    const NAME: &'static str = "csv_history";

    async fn new() -> Result<Self> {
        let _ = default_histpath()?; // Ensure the file exists
        Ok(Self)
    }

    async fn entries(&mut self) -> Result<usize> {
        let file = File::open(default_histpath()?)?;
        let reader = BufReader::new(file);
        Ok(reader.lines().count() - 1) // Exclude header row
    }

    async fn load(self, h: &mut impl Loader) -> Result<()> {
        let file = File::open(default_histpath()?)?;
        let mut reader = Reader::from_reader(file);

        for result in reader.records() {
            let record = result?;
            if let (Some(timestamp), Some(duration), Some(command)) =
                (record.get(0), record.get(1), record.get(2))
            {
                let timestamp = timestamp
                    .parse::<i64>()
                    .ok()
                    .and_then(|t| OffsetDateTime::from_unix_timestamp(t).ok())
                    .unwrap_or_else(OffsetDateTime::now_utc);
                let duration = duration.parse::<i64>().map_or(-1, |t| t * 1_000_000_000);

                let imported = History::import()
                    .timestamp(timestamp)
                    .command(command.trim().to_string())
                    .duration(duration);

                h.push(imported.build().into()).await?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::import::tests::TestLoader;
    use itertools::assert_equal;
    use std::fs;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[tokio::test]
    async fn test_parse_file() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "timestamp,duration,command").unwrap();
        writeln!(file, "1613322469,10,cargo install atuin").unwrap();
        writeln!(file, "1613322470,5,ls -la").unwrap();
        writeln!(file, "1613322471,3,git status").unwrap();

        let path = file.path().to_path_buf();
        fs::copy(&path, HISTORY_FILE).unwrap();

        let mut importer = Csv::new().await.unwrap();
        assert_eq!(importer.entries().await.unwrap(), 3);

        let mut loader = TestLoader::default();
        importer.load(&mut loader).await.unwrap();

        assert_equal(
            loader.buf.iter().map(|h| h.command.as_str()),
            ["cargo install atuin", "ls -la", "git status"],
        );

        fs::remove_file(HISTORY_FILE).unwrap();
    }
}
