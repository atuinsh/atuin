// import old shell history!
// automatically hoover up all that we can find

use std::borrow::Cow;
use std::path::PathBuf;

use async_trait::async_trait;
use eyre::{Result, eyre};
use jiff::{SignedDuration, Timestamp};

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
    // oh-my-zsh sets HISTFILE=~/.zhistory.
    // zsh has no default value for HISTFILE, and zsh-newuser-install proposes
    // $ZDOTDIR/.histfile or ~/.histfile when ZDOTDIR is unset:
    // https://github.com/zsh-users/zsh/blob/master/Functions/Newuser/zsh-newuser-install#L794
    const CANDIDATES: [&str; 3] = [".zhistory", ".zsh_history", ".histfile"];

    let zdotdir = atuin_common::env::var_nonempty("ZDOTDIR").map(PathBuf::from);
    let home_dir = atuin_common::dirs::try_home_dir().ok();

    if zdotdir.is_none() && home_dir.is_none() {
        return Err(eyre!("could not find home directory"));
    }

    for dir in zdotdir.into_iter().chain(home_dir) {
        for candidate in CANDIDATES {
            let histpath = dir.join(candidate);
            if histpath.is_file() {
                return Ok(histpath);
            }
        }
    }

    Err(eyre!("Could not find history file. Try setting and exporting $HISTFILE"))
}

/// Represents a line of zsh history.
struct Entry {
    pub command: String,
    pub timestamp: Option<Timestamp>,
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

        let time = time.parse::<i64>().ok().and_then(|t| Timestamp::from_second(t).ok());

        // use nanos, because why the hell not? we won't display them.
        // saturate rather than overflow on an implausible duration
        let duration = duration.parse::<i64>().map_or(-1, |t| t.saturating_mul(1_000_000_000));

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
            // we can skip past things like invalid utf8
            let Some(s) = unmetafy(b) else {
                continue;
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
            .unwrap_or_else(|| (entries.len(), Timestamp::now()));

        let timestamp_increment = SignedDuration::from_millis(1);
        let backfill =
            timestamp_increment * i32::try_from(commands_until_timestamp).unwrap_or(i32::MAX);
        // a timestamp near the start of the representable range would underflow
        let mut timestamp = first_timestamp.checked_sub(backfill).unwrap_or(first_timestamp);

        for entry in entries {
            if let Some(time) = entry.timestamp {
                timestamp = time;
            } else {
                // a timestamp near the end of the representable range would overflow
                timestamp = timestamp.checked_add(timestamp_increment).unwrap_or(timestamp);
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
                s.push(*ch);
            }
        }
        String::from_utf8(s).ok().map(Cow::Owned)
    } else {
        std::str::from_utf8(line).ok().map(Cow::Borrowed)
    }
}

#[cfg(test)]
mod test {
    use atuin_common::env::MockEnv;
    use itertools::assert_equal;
    use rstest::rstest;

    use super::*;
    use crate::import::tests::TestLoader;

    #[rstest]
    #[case::zero_duration(
        ": 1613322469:0;cargo install atuin",
        "cargo install atuin",
        Some(1_613_322_469),
        Some(0)
    )]
    #[case::multi_semicolon(
        ": 1613322469:10;cargo install atuin;cargo update",
        "cargo install atuin;cargo update",
        Some(1_613_322_469),
        Some(10_000_000_000)
    )]
    #[case::unicode_command(
        ": 1613322469:10;cargo :b̷i̶t̴r̵o̴t̴ ̵i̷s̴ ̷r̶e̵a̸l̷",
        "cargo :b̷i̶t̴r̵o̴t̴ ̵i̷s̴ ̷r̶e̵a̸l̷",
        Some(1_613_322_469),
        Some(10_000_000_000)
    )]
    #[case::trailing_newline_escape(
        ": 1613322469:10;cargo install \\n atuin\n",
        "cargo install \\n atuin",
        Some(1_613_322_469),
        Some(10_000_000_000)
    )]
    fn parse_extended_simple(
        #[case] line: &str,
        #[case] command: &str,
        #[case] timestamp: Option<i64>,
        #[case] duration: Option<i64>,
    ) {
        let parsed = Entry::parse(line);

        assert_eq!(parsed.command, command);
        assert_eq!(parsed.duration, duration);
        assert_eq!(parsed.timestamp.map(|t| t.as_second()), timestamp);
    }

    /// Lines that are not valid extended history must not panic. Anything that
    /// does not match `: <start>:<duration>;<command>` is a bare command line;
    /// values that do match but cannot be represented are dropped or saturated.
    #[rstest]
    #[case::no_colon(": not-extended", ": not-extended", None, None)]
    #[case::no_semicolon(": 1613322469:0", ": 1613322469:0", None, None)]
    #[case::out_of_range_time(": 999999999999999:0;echo hello", "echo hello", None, Some(0))]
    #[case::duration_saturates(
        ": 1613322469:9223372036854775807;echo hello",
        "echo hello",
        Some(1_613_322_469),
        Some(i64::MAX)
    )]
    fn parse_malformed_extended_lines(
        #[case] line: &str,
        #[case] command: &str,
        #[case] timestamp: Option<i64>,
        #[case] duration: Option<i64>,
    ) {
        let parsed = Entry::parse(line);

        assert_eq!(parsed.command, command);
        assert_eq!(parsed.timestamp.map(Timestamp::as_second), timestamp);
        assert_eq!(parsed.duration, duration);
    }

    #[rstest]
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

        assert_equal(loader.buf.iter().map(|h| h.command.as_str()), [
            "cargo install atuin",
            "cargo install atuin; \\\ncargo update",
            "cargo :b̷i̶t̴r̵o̴t̴ ̵i̷s̴ ̷r̶e̵a̸l̷",
        ]);
    }

    #[rstest]
    #[tokio::test]
    async fn timestamp_near_range_start_does_not_panic_on_backfill() {
        // first timestamp is near the minimum representable instant, preceded by an
        // untimestamped command; backfilling before it must not underflow
        let bytes = b"cargo install atuin\n: -377705023201:0;cargo update\n".to_vec();

        let mut zsh = Zsh { bytes };
        assert_eq!(zsh.entries().await.unwrap(), 2);

        let mut loader = TestLoader::default();
        zsh.load(&mut loader).await.unwrap();

        assert_equal(loader.buf.iter().map(|h| h.command.as_str()), [
            "cargo install atuin",
            "cargo update",
        ]);
    }

    #[rstest]
    #[tokio::test]
    async fn timestamp_near_range_end_does_not_panic_on_increment() {
        // first timestamp is the maximum representable instant (253402207200 is the
        // last second `Timestamp` can represent, i.e. 9999-12-31 23:59:59 UTC).
        // 1000 untimestamped commands walk the 1ms increment past .999 and off the
        // end of the representable range.
        let commands: Vec<String> = (0..1_000).map(|i| format!("cmd-{i}")).collect();
        let bytes = format!(": 253402207200:0;first\n{}\n", commands.join("\n")).into_bytes();

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
        assert_eq!(loader.buf[0].timestamp.as_second(), 253_402_207_200);
        // the increment saturates at the maximum representable instant rather than
        // wrapping or resetting to the epoch
        assert_eq!(loader.buf.last().unwrap().timestamp.as_second(), 253_402_207_200);
    }

    #[rstest]
    #[tokio::test]
    async fn test_parse_metafied() {
        let bytes =
            b"echo \xe4\xbd\x83\x80\xe5\xa5\xbd\nls ~/\xe9\x83\xbf\xb3\xe4\xb9\x83\xb0\n".to_vec();

        let mut zsh = Zsh { bytes };
        assert_eq!(zsh.entries().await.unwrap(), 2);

        let mut loader = TestLoader::default();
        zsh.load(&mut loader).await.unwrap();

        assert_equal(loader.buf.iter().map(|h| h.command.as_str()), ["echo 你好", "ls ~/音乐"]);
    }

    #[rstest]
    #[case::zhistory(".zhistory")]
    #[case::zsh_history(".zsh_history")]
    #[case::histfile(".histfile")]
    fn default_histpath_finds_candidates_in_zdotdir(#[case] filename: &str) {
        let home = tempfile::tempdir().unwrap();
        let zdotdir = tempfile::tempdir().unwrap();
        let expected = zdotdir.path().join(filename);
        std::fs::write(&expected, b"echo hello\n").unwrap();

        let env = MockEnv::install();
        env.set("HOME", home.path());
        env.set("ZDOTDIR", zdotdir.path());

        assert_eq!(default_histpath().unwrap(), expected);
    }

    #[rstest]
    fn default_histpath_prefers_zdotdir_over_home() {
        let home = tempfile::tempdir().unwrap();
        let zdotdir = tempfile::tempdir().unwrap();
        std::fs::write(home.path().join(".zhistory"), b"echo home\n").unwrap();
        let expected = zdotdir.path().join(".zsh_history");
        std::fs::write(&expected, b"echo zdotdir\n").unwrap();

        let env = MockEnv::install();
        env.set("HOME", home.path());
        env.set("ZDOTDIR", zdotdir.path());

        assert_eq!(default_histpath().unwrap(), expected);
    }

    #[rstest]
    fn default_histpath_falls_back_to_home_when_zdotdir_has_no_history() {
        let home = tempfile::tempdir().unwrap();
        let zdotdir = tempfile::tempdir().unwrap();
        let expected = home.path().join(".zsh_history");
        std::fs::write(&expected, b"echo home\n").unwrap();

        let env = MockEnv::install();
        env.set("HOME", home.path());
        env.set("ZDOTDIR", zdotdir.path());

        assert_eq!(default_histpath().unwrap(), expected);
    }

    #[rstest]
    fn default_histpath_ignores_empty_zdotdir() {
        let home = tempfile::tempdir().unwrap();
        let expected = home.path().join(".zhistory");
        std::fs::write(&expected, b"echo home\n").unwrap();

        let env = MockEnv::install();
        env.set("HOME", home.path());
        env.set("ZDOTDIR", "");

        assert_eq!(default_histpath().unwrap(), expected);
    }

    #[rstest]
    fn default_histpath_works_with_zdotdir_and_no_home() {
        let zdotdir = tempfile::tempdir().unwrap();
        let expected = zdotdir.path().join(".histfile");
        std::fs::write(&expected, b"echo zdotdir\n").unwrap();

        let env = MockEnv::install();
        env.set("ZDOTDIR", zdotdir.path());

        assert_eq!(default_histpath().unwrap(), expected);
    }

    #[rstest]
    fn default_histpath_errors_when_no_directories_set() {
        let _env = MockEnv::install();

        assert!(default_histpath().is_err());
    }

    #[rstest]
    fn default_histpath_errors_when_no_history_file_exists() {
        let home = tempfile::tempdir().unwrap();
        let zdotdir = tempfile::tempdir().unwrap();

        let env = MockEnv::install();
        env.set("HOME", home.path());
        env.set("ZDOTDIR", zdotdir.path());

        assert!(default_histpath().is_err());
    }

    #[rstest]
    fn histfile_takes_precedence_over_zdotdir() {
        let home = tempfile::tempdir().unwrap();
        let zdotdir = tempfile::tempdir().unwrap();
        let custom = tempfile::tempdir().unwrap();
        std::fs::write(zdotdir.path().join(".zsh_history"), b"echo zdotdir\n").unwrap();
        let expected = custom.path().join("custom_history");
        std::fs::write(&expected, b"echo custom\n").unwrap();

        let env = MockEnv::install();
        env.set("HOME", home.path());
        env.set("ZDOTDIR", zdotdir.path());
        env.set("HISTFILE", &expected);

        assert_eq!(get_histfile_path(default_histpath).unwrap(), expected);
    }
}
