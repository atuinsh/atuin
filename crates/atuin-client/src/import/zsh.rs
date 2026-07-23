// import old shell history!
// automatically hoover up all that we can find

use std::borrow::Cow;
use std::path::PathBuf;

use async_trait::async_trait;
use directories::UserDirs;
use eyre::{Result, eyre};
use time::{Duration, OffsetDateTime};

use super::{Importer, Loader, get_histfile_path, unix_byte_lines};
use crate::history::History;
use crate::history::builder::HistoryImported;
use crate::import::read_to_end;

#[derive(Debug)]
pub struct Zsh {
    bytes: Vec<u8>,
}

impl Zsh {
    fn num_entries(&self) -> usize {
        super::count_lines(&self.bytes)
    }
}

fn default_histpath() -> Result<PathBuf> {
    // oh-my-zsh sets HISTFILE=~/.zhistory
    // zsh has no default value for this var, but uses ~/.zhistory.
    // zsh-newuser-install propose as default .histfile https://github.com/zsh-users/zsh/blob/master/Functions/Newuser/zsh-newuser-install#L794
    // we could maybe be smarter about this in the future :)
    let user_dirs = UserDirs::new().ok_or_else(|| eyre!("could not find user directories"))?;
    let home_dir = user_dirs.home_dir();

    let mut candidates = [".zhistory", ".zsh_history", ".histfile"].iter();
    loop {
        match candidates.next() {
            Some(candidate) => {
                let histpath = home_dir.join(candidate);
                if histpath.exists() {
                    break Ok(histpath);
                }
            }
            None => {
                break Err(eyre!(
                    "Could not find history file. Try setting and exporting $HISTFILE"
                ));
            }
        }
    }
}

/// Represents a line of zsh history.
struct Entry {
    pub command: String,
    pub timestamp: Option<OffsetDateTime>,
    /// Nanoseconds
    pub duration: Option<i64>,
}

impl Entry {
    pub fn parse(line: &str) -> Self {
        // extended history looks like `: <start>:<duration>;<command>`.
        // anything that does not match that shape is a bare command line
        let extended = line
            .strip_prefix(": ")
            .and_then(|rest| rest.split_once(':'))
            .and_then(|(time, rest)| rest.split_once(';').map(|(dur, cmd)| (time, dur, cmd)));

        let Some((time, duration, command)) = extended else {
            return Self {
                command: line.trim_end().to_owned(),
                timestamp: None,
                duration: None,
            };
        };

        let time = time
            .parse::<i64>()
            .ok()
            .and_then(|t| OffsetDateTime::from_unix_timestamp(t).ok());

        // use nanos, because why the hell not? we won't display them.
        // saturate rather than overflow on an implausible duration
        let duration = duration
            .parse::<i64>()
            .map_or(-1, |t| t.saturating_mul(1_000_000_000));

        Self {
            command: command.trim_end().to_owned(),
            timestamp: time,
            duration: Some(duration),
        }
    }
}

#[async_trait]
impl Importer for Zsh {
    const NAME: &'static str = "zsh";

    async fn new() -> Result<Self> {
        let bytes = read_to_end(get_histfile_path(default_histpath)?)?;
        Ok(Self { bytes })
    }

    async fn entries(&mut self) -> Result<usize> {
        Ok(self.num_entries())
    }

    async fn load(self, h: &mut impl Loader) -> Result<()> {
        let mut line = String::new();
        let mut entries = Vec::with_capacity(self.num_entries());

        for b in unix_byte_lines(&self.bytes) {
            let s = match unmetafy(b) {
                Some(s) => s,
                _ => continue, // we can skip past things like invalid utf8
            };

            if let Some(s) = s.strip_suffix('\\') {
                line.push_str(s);
                line.push('\n');
            } else {
                line.push_str(&s);
                entries.push(Entry::parse(&line));
                line.clear();
            }
        }

        // Similar approach to preserving order as the Bash importer.
        let (commands_until_timestamp, first_timestamp) = entries
            .iter()
            .enumerate()
            .find_map(|(i, entry)| entry.timestamp.map(|t| (i + 1, t)))
            .unwrap_or_else(|| (entries.len(), OffsetDateTime::now_utc()));

        let timestamp_increment = Duration::milliseconds(1);
        let backfill =
            u32::try_from(commands_until_timestamp).unwrap_or(u32::MAX) * timestamp_increment;
        // a timestamp near the start of the representable range would underflow
        let mut timestamp = first_timestamp
            .checked_sub(backfill)
            .unwrap_or(first_timestamp);

        for entry in entries {
            if let Some(time) = entry.timestamp {
                timestamp = time;
            } else {
                // a timestamp near the end of the representable range would overflow
                timestamp = timestamp
                    .checked_add(timestamp_increment)
                    .unwrap_or(timestamp);
            }

            let imported = History::import()
                .shell("zsh")
                .timestamp(timestamp)
                .duration(entry.duration.unwrap_or(HistoryImported::DEFAULT_DURATION))
                .command(entry.command)
                .build();
            h.push(imported.into()).await?;
        }
        Ok(())
    }
}

fn unmetafy(line: &[u8]) -> Option<Cow<'_, str>> {
    if line.contains(&0x83) {
        let mut s = Vec::with_capacity(line.len());
        let mut is_meta = false;
        for ch in line {
            if *ch == 0x83 {
                is_meta = true;
            } else if is_meta {
                is_meta = false;
                s.push(*ch ^ 32);
            } else {
                s.push(*ch)
            }
        }
        String::from_utf8(s).ok().map(Cow::Owned)
    } else {
        std::str::from_utf8(line).ok().map(Cow::Borrowed)
    }
}

#[cfg(test)]
mod test {
    use itertools::assert_equal;

    use crate::import::tests::TestLoader;

    use super::*;

    #[test]
    fn test_parse_extended_simple() {
        let parsed = Entry::parse(": 1613322469:0;cargo install atuin");

        assert_eq!(parsed.command, "cargo install atuin");
        assert_eq!(parsed.duration, Some(0));
        assert_eq!(
            parsed.timestamp.unwrap(),
            OffsetDateTime::from_unix_timestamp(1_613_322_469).unwrap()
        );

        let parsed = Entry::parse(": 1613322469:10;cargo install atuin;cargo update");

        assert_eq!(parsed.command, "cargo install atuin;cargo update");
        assert_eq!(parsed.duration, Some(10_000_000_000));
        assert_eq!(
            parsed.timestamp.unwrap(),
            OffsetDateTime::from_unix_timestamp(1_613_322_469).unwrap()
        );

        let parsed = Entry::parse(": 1613322469:10;cargo :b̷i̶t̴r̵o̴t̴ ̵i̷s̴ ̷r̶e̵a̸l̷");

        assert_eq!(parsed.command, "cargo :b̷i̶t̴r̵o̴t̴ ̵i̷s̴ ̷r̶e̵a̸l̷");
        assert_eq!(parsed.duration, Some(10_000_000_000));
        assert_eq!(
            parsed.timestamp.unwrap(),
            OffsetDateTime::from_unix_timestamp(1_613_322_469).unwrap()
        );

        let parsed = Entry::parse(": 1613322469:10;cargo install \\n atuin\n");

        assert_eq!(parsed.command, "cargo install \\n atuin");
        assert_eq!(parsed.duration, Some(10_000_000_000));
        assert_eq!(
            parsed.timestamp.unwrap(),
            OffsetDateTime::from_unix_timestamp(1_613_322_469).unwrap()
        );
    }

    #[test]
    fn parse_malformed_extended_lines() {
        // none of these are valid extended history, and none may panic;
        // they fall through to being treated as a bare command
        let no_colon = Entry::parse(": not-extended");
        assert_eq!(no_colon.command, ": not-extended");
        assert_eq!(no_colon.timestamp, None);
        assert_eq!(no_colon.duration, None);

        let no_semicolon = Entry::parse(": 1613322469:0");
        assert_eq!(no_semicolon.command, ": 1613322469:0");
        assert_eq!(no_semicolon.timestamp, None);
        assert_eq!(no_semicolon.duration, None);
    }

    #[test]
    fn parse_out_of_range_extended_values() {
        // command survives, timestamp is dropped
        let bad_time = Entry::parse(": 999999999999999:0;echo hello");
        assert_eq!(bad_time.command, "echo hello");
        assert_eq!(bad_time.timestamp, None);

        // seconds -> nanos must saturate rather than overflow
        let bad_duration = Entry::parse(": 1613322469:9223372036854775807;echo hello");
        assert_eq!(bad_duration.command, "echo hello");
        assert_eq!(bad_duration.duration, Some(i64::MAX));
    }

    #[tokio::test]
    async fn test_parse_file() {
        let bytes = r": 1613322469:0;cargo install atuin
: 1613322469:10;cargo install atuin; \\
cargo update
: 1613322469:10;cargo :b̷i̶t̴r̵o̴t̴ ̵i̷s̴ ̷r̶e̵a̸l̷
"
        .as_bytes()
        .to_owned();

        let mut zsh = Zsh { bytes };
        assert_eq!(zsh.entries().await.unwrap(), 4);

        let mut loader = TestLoader::default();
        zsh.load(&mut loader).await.unwrap();

        assert_equal(
            loader.buf.iter().map(|h| h.command.as_str()),
            [
                "cargo install atuin",
                "cargo install atuin; \\\ncargo update",
                "cargo :b̷i̶t̴r̵o̴t̴ ̵i̷s̴ ̷r̶e̵a̸l̷",
            ],
        );
    }

    #[tokio::test]
    async fn timestamp_near_range_start_does_not_panic_on_backfill() {
        // first timestamp is near the minimum representable instant, preceded by an
        // untimestamped command; backfilling before it must not underflow
        let bytes = b"cargo install atuin\n: -377705116800:0;cargo update\n".to_vec();

        let mut zsh = Zsh { bytes };
        assert_eq!(zsh.entries().await.unwrap(), 2);

        let mut loader = TestLoader::default();
        zsh.load(&mut loader).await.unwrap();

        assert_equal(
            loader.buf.iter().map(|h| h.command.as_str()),
            ["cargo install atuin", "cargo update"],
        );
    }

    #[tokio::test]
    async fn timestamp_near_range_end_does_not_panic_on_increment() {
        // first timestamp is the maximum representable instant (253402300799 is the
        // last second `OffsetDateTime` can represent, i.e. 9999-12-31 23:59:59 UTC).
        // 1000 untimestamped commands walk the 1ms increment past .999 and off the
        // end of the representable range.
        let commands: Vec<String> = (0..1_000).map(|i| format!("cmd-{i}")).collect();
        let bytes = format!(": 253402300799:0;first\n{}\n", commands.join("\n")).into_bytes();

        let mut zsh = Zsh { bytes };
        assert_eq!(zsh.entries().await.unwrap(), commands.len() + 1);

        let mut loader = TestLoader::default();
        zsh.load(&mut loader).await.unwrap();

        let mut expected = vec!["first".to_string()];
        expected.extend(commands);
        assert_equal(
            loader.buf.iter().map(|h| h.command.as_str()),
            expected.iter().map(String::as_str),
        );

        // the first entry's timestamp must actually resolve; without this, every
        // entry would silently fall back to untimestamped and the overflow this
        // test targets would never be exercised
        assert_eq!(loader.buf[0].timestamp.unix_timestamp(), 253_402_300_799);
        // the increment saturates at the maximum representable instant rather than
        // wrapping or resetting to the epoch
        assert_eq!(
            loader.buf.last().unwrap().timestamp.unix_timestamp(),
            253_402_300_799
        );
    }

    #[tokio::test]
    async fn test_parse_metafied() {
        let bytes =
            b"echo \xe4\xbd\x83\x80\xe5\xa5\xbd\nls ~/\xe9\x83\xbf\xb3\xe4\xb9\x83\xb0\n".to_vec();

        let mut zsh = Zsh { bytes };
        assert_eq!(zsh.entries().await.unwrap(), 2);

        let mut loader = TestLoader::default();
        zsh.load(&mut loader).await.unwrap();

        assert_equal(
            loader.buf.iter().map(|h| h.command.as_str()),
            ["echo 你好", "ls ~/音乐"],
        );
    }
}
