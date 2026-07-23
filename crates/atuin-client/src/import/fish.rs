// import old shell history!
// automatically hoover up all that we can find

use std::path::PathBuf;

use async_trait::async_trait;
use directories::BaseDirs;
use eyre::{Result, eyre};
use time::OffsetDateTime;

use super::{Importer, Loader, unix_byte_lines};
use crate::history::History;
use crate::import::read_to_end;

#[derive(Debug)]
pub struct Fish {
    bytes: Vec<u8>,
}

/// see <https://fishshell.com/docs/current/interactive.html#searchable-command-history>
fn default_histpath() -> Result<PathBuf> {
    let base = BaseDirs::new().ok_or_else(|| eyre!("could not determine data directory"))?;
    let data = std::env::var("XDG_DATA_HOME").map_or_else(
        |_| base.home_dir().join(".local").join("share"),
        PathBuf::from,
    );

    // fish supports multiple history sessions
    // If `fish_history` var is missing, or set to `default`, use `fish` as the session
    let session = std::env::var("fish_history").unwrap_or_else(|_| String::from("fish"));
    let session = if session == "default" {
        String::from("fish")
    } else {
        session
    };

    let mut histpath = data.join("fish");
    histpath.push(format!("{session}_history"));

    if histpath.exists() {
        Ok(histpath)
    } else {
        Err(eyre!("Could not find history file."))
    }
}

#[async_trait]
impl Importer for Fish {
    const NAME: &'static str = "fish";

    async fn new() -> Result<Self> {
        let bytes = read_to_end(default_histpath()?)?;
        Ok(Self { bytes })
    }

    async fn entries(&mut self) -> Result<usize> {
        Ok(super::count_lines(&self.bytes))
    }

    async fn load(self, loader: &mut impl Loader) -> Result<()> {
        let now = OffsetDateTime::now_utc();
        let mut time: Option<OffsetDateTime> = None;
        let mut cmd: Option<String> = None;

        let mut process_cmd = async |cmd: &mut Option<String>, time: Option<OffsetDateTime>| {
            let Some(cmd) = cmd.take() else {
                return Ok(());
            };

            let time = time.unwrap_or(now);
            let entry = History::import().shell("fish").timestamp(time).command(cmd);

            loader.push(entry.build().into()).await
        };

        for b in unix_byte_lines(&self.bytes) {
            let s = match std::str::from_utf8(b) {
                Ok(s) => s,
                Err(_) => continue, // we can skip past things like invalid utf8
            };

            if let Some(c) = s.strip_prefix("- cmd: ") {
                // first, we must deal with the prev cmd
                process_cmd(&mut cmd, time).await?;

                // using raw strings to avoid needing escaping.
                // replaces double backslashes with single backslashes
                let c = c.replace(r"\\", r"\");
                // replaces escaped newlines
                let c = c.replace(r"\n", "\n");
                // TODO: any other escape characters?

                cmd = Some(c);
            } else if let Some(t) = s.strip_prefix("  when: ") {
                // ignore the line if it is not an int, or is outside the range we can
                // represent - keeping the previous entry's timestamp preserves ordering,
                // and a corrupt entry must not abort the import
                time = t
                    .parse::<i64>()
                    .ok()
                    .and_then(|t| OffsetDateTime::from_unix_timestamp(t).ok())
                    .or(time);
            } else {
                // ... ignore paths lines
            }
        }

        // we might have a trailing cmd
        process_cmd(&mut cmd, time).await?;
        Ok(())
    }
}

#[cfg(test)]
mod test {

    use crate::import::{Importer, tests::TestLoader};

    use super::Fish;

    #[tokio::test]
    async fn parse_out_of_range_timestamp() {
        // A corrupt `when:` must degrade that one entry, not abort the import.
        // https://github.com/atuinsh/atuin/issues/938
        let bytes = r"- cmd: echo before
  when: 1639162832
- cmd: echo corrupt
  when: 999999999999999
- cmd: echo after
  when: 1639162851
"
        .as_bytes()
        .to_owned();

        let fish = Fish { bytes };
        let mut loader = TestLoader::default();
        fish.load(&mut loader).await.expect("import must not fail");

        let commands: Vec<&str> = loader.buf.iter().map(|h| h.command.as_str()).collect();
        assert_eq!(commands, ["echo before", "echo corrupt", "echo after"]);

        // the corrupt entry inherits its predecessor's timestamp, so ordering holds
        assert_eq!(loader.buf[0].timestamp.unix_timestamp(), 1_639_162_832);
        assert_eq!(loader.buf[1].timestamp.unix_timestamp(), 1_639_162_832);
        assert_eq!(loader.buf[2].timestamp.unix_timestamp(), 1_639_162_851);
    }

    #[tokio::test]
    async fn parse_complex() {
        // complicated input with varying contents and escaped strings.
        let bytes = r#"- cmd: history --help
  when: 1639162832
- cmd: cat ~/.bash_history
  when: 1639162851
  paths:
    - ~/.bash_history
- cmd: ls ~/.local/share/fish/fish_history
  when: 1639162890
  paths:
    - ~/.local/share/fish/fish_history
- cmd: cat ~/.local/share/fish/fish_history
  when: 1639162893
  paths:
    - ~/.local/share/fish/fish_history
ERROR
- CORRUPTED: ENTRY
  CONTINUE:
    - AS
    - NORMAL
- cmd: echo "foo" \\\n'bar' baz
  when: 1639162933
- cmd: cat ~/.local/share/fish/fish_history
  when: 1639162939
  paths:
    - ~/.local/share/fish/fish_history
- cmd: echo "\\"" \\\\ "\\\\"
  when: 1639163063
- cmd: cat ~/.local/share/fish/fish_history
  when: 1639163066
  paths:
    - ~/.local/share/fish/fish_history
"#
        .as_bytes()
        .to_owned();

        let fish = Fish { bytes };

        let mut loader = TestLoader::default();
        fish.load(&mut loader).await.unwrap();
        let mut history = loader.buf.into_iter();

        // simple wrapper for fish history entry
        macro_rules! fishtory {
            ($timestamp:expr_2021, $command:expr_2021) => {
                let h = history.next().expect("missing entry in history");
                assert_eq!(h.command.as_str(), $command);
                assert_eq!(h.timestamp.unix_timestamp(), $timestamp);
            };
        }

        fishtory!(1639162832, "history --help");
        fishtory!(1639162851, "cat ~/.bash_history");
        fishtory!(1639162890, "ls ~/.local/share/fish/fish_history");
        fishtory!(1639162893, "cat ~/.local/share/fish/fish_history");
        fishtory!(1639162933, "echo \"foo\" \\\n'bar' baz");
        fishtory!(1639162939, "cat ~/.local/share/fish/fish_history");
        fishtory!(1639163063, r#"echo "\"" \\ "\\""#);
        fishtory!(1639163066, "cat ~/.local/share/fish/fish_history");
    }
}
