use std::{path::PathBuf, str};

use async_trait::async_trait;
use directories::UserDirs;
use eyre::{eyre, Result};
use time::{macros::format_description, OffsetDateTime, PrimitiveDateTime};

use super::{get_histpath, unix_byte_lines, Importer, Loader};
use crate::history::History;
use crate::import::read_to_end;

#[derive(Debug)]
pub struct Replxx {
    bytes: Vec<u8>,
}

fn default_histpath() -> Result<PathBuf> {
    let user_dirs = UserDirs::new().ok_or_else(|| eyre!("could not find user directories"))?;
    let home_dir = user_dirs.home_dir();

    // There is no default histfile for replxx.
    // For simplicity let's use the most common one.
    Ok(home_dir.join(".histfile"))
}

#[async_trait]
impl Importer for Replxx {
    const NAME: &'static str = "replxx";

    async fn new() -> Result<Self> {
        let bytes = read_to_end(get_histpath(default_histpath)?)?;
        Ok(Self { bytes })
    }

    async fn entries(&mut self) -> Result<usize> {
        Ok(super::count_lines(&self.bytes) / 2)
    }

    async fn load(self, h: &mut impl Loader) -> Result<()> {
        let mut timestamp = OffsetDateTime::UNIX_EPOCH;

        for b in unix_byte_lines(&self.bytes) {
            let s = std::str::from_utf8(b)?;
            match try_parse_line_as_timestamp(s) {
                Some(t) => timestamp = t,
                None => {
                    // replxx uses ETB character (0x17) as line breaker
                    let cmd = s.replace('\u{0017}', "\n");
                    let imported = History::import().timestamp(timestamp).command(cmd);

                    h.push(imported.build().into()).await?;
                }
            }
        }

        Ok(())
    }
}

fn try_parse_line_as_timestamp(line: &str) -> Option<OffsetDateTime> {
    // replxx history date time format: ### yyyy-mm-dd hh:mm:ss.xxx
    let date_time_str = line.strip_prefix("### ")?;
    let format =
        format_description!("[year]-[month]-[day] [hour]:[minute]:[second].[subsecond digits:3]");

    let primitive_date_time = PrimitiveDateTime::parse(date_time_str, format).ok()?;
    // There is no safe way to get local time offset.
    // For simplicity let's just assume UTC.
    Some(primitive_date_time.assume_utc())
}

#[cfg(test)]
mod test {

    use crate::import::{tests::TestLoader, Importer};

    use super::Replxx;

    #[tokio::test]
    async fn parse_complex() {
        let bytes = r#"### 2024-02-10 22:16:28.302
select * from remote('127.0.0.1:20222', view(select 1))
### 2024-02-10 22:16:36.919
select * from numbers(10)
### 2024-02-10 22:16:41.710
select * from system.numbers
### 2024-02-10 22:19:28.655
select 1
### 2024-02-22 11:15:33.046
CREATE TABLE test( stamp DateTime('UTC'))ENGINE = MergeTreePARTITION BY toDate(stamp)order by tuple() as select toDateTime('2020-01-01')+number*60 from numbers(80000);
"#
        .as_bytes()
        .to_owned();

        let replxx = Replxx { bytes };

        let mut loader = TestLoader::default();
        replxx.load(&mut loader).await.unwrap();
        let mut history = loader.buf.into_iter();

        // simple wrapper for replxx history entry
        macro_rules! history {
            ($timestamp:expr, $command:expr) => {
                let h = history.next().expect("missing entry in history");
                assert_eq!(h.command.as_str(), $command);
                assert_eq!(h.timestamp.unix_timestamp(), $timestamp);
            };
        }

        history!(
            1707603388,
            "select * from remote('127.0.0.1:20222', view(select 1))"
        );
        history!(1707603396, "select * from numbers(10)");
        history!(1707603401, "select * from system.numbers");
        history!(1707603568, "select 1");
        history!(1708600533, "CREATE TABLE test\n( stamp DateTime('UTC'))\nENGINE = MergeTree\nPARTITION BY toDate(stamp)\norder by tuple() as select toDateTime('2020-01-01')+number*60 from numbers(80000);");
    }
}
