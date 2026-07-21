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
        if let Some(rest) = line.strip_prefix(": ") {
            let (time, rest) = rest.split_once(':').unwrap();
            let (duration, command) = rest.split_once(';').unwrap();
            let time = time
                .parse::<i64>()
                .ok()
                .and_then(|t| OffsetDateTime::from_unix_timestamp(t).ok());

            // use nanos, because why the hell not? we won't display them.
            let duration = duration.parse::<i64>().map_or(-1, |t| t * 1_000_000_000);
            Self {
                command: command.trim_end().to_owned(),
                timestamp: time,
                duration: Some(duration),
            }
        } else {
            Self {
                command: line.trim_end().to_owned(),
                timestamp: None,
                duration: None,
            }
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
        let mut timestamp = first_timestamp
            - u32::try_from(commands_until_timestamp).unwrap_or(u32::MAX) * timestamp_increment;

        for entry in entries {
            if let Some(time) = entry.timestamp {
                timestamp = time;
            } else {
                timestamp += timestamp_increment;
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
