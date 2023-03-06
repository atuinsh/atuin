// import old shell history!
// automatically hoover up all that we can find

use std::{
    fs::{self},
    path::{Path, PathBuf},
};

use async_trait::async_trait;
use chrono::{prelude::*, Utc};
use directories::BaseDirs;
use eyre::{bail, Result};

use super::{unix_byte_lines, Importer, Loader};
use crate::history::History;

#[derive(Debug)]
pub struct Fish {
    bytes: Vec<u8>,
}

#[async_trait]
impl Importer for Fish {
    const NAME: &'static str = "fish";

    /// See https://fishshell.com/docs/current/interactive.html#searchable-command-history
    fn default_source_path() -> Result<PathBuf> {
        let Some(base_dir) = BaseDirs::new() else {
            bail!("could not determine data directory");
        };
        let data_dir = base_dir.data_local_dir();

        // fish supports multiple history sessions
        // If `fish_history` var is missing, or set to `default`, use `fish` as the session
        let fish_history_var = std::env::var("fish_history");
        let session = match fish_history_var.as_ref().map(|s| s.as_str()) {
            Err(_) | Ok("default") => "fish",
            Ok(other) => other,
        };

        let path = data_dir.join("fish").join(format!("{session}_history"));
        Ok(path)
    }

    async fn new(source: &Path) -> Result<Self> {
        let bytes = fs::read(source)?;
        Ok(Self { bytes })
    }

    async fn entries(&mut self) -> Result<usize> {
        Ok(super::count_lines(&self.bytes))
    }

    async fn load(self, loader: &mut impl Loader) -> Result<()> {
        let now = Utc::now();
        let mut time: Option<DateTime<Utc>> = None;
        let mut cmd: Option<String> = None;

        for b in unix_byte_lines(&self.bytes) {
            let s = match std::str::from_utf8(b) {
                Ok(s) => s,
                Err(_) => continue, // we can skip past things like invalid utf8
            };

            if let Some(c) = s.strip_prefix("- cmd: ") {
                // first, we must deal with the prev cmd
                if let Some(cmd) = cmd.take() {
                    let time = time.unwrap_or(now);

                    loader
                        .push(History::new(
                            time,
                            cmd,
                            "unknown".into(),
                            -1,
                            -1,
                            None,
                            None,
                        ))
                        .await?;
                }

                // using raw strings to avoid needing escaping.
                // replaces double backslashes with single backslashes
                let c = c.replace(r"\\", r"\");
                // replaces escaped newlines
                let c = c.replace(r"\n", "\n");
                // TODO: any other escape characters?

                cmd = Some(c);
            } else if let Some(t) = s.strip_prefix("  when: ") {
                // if t is not an int, just ignore this line
                if let Ok(t) = t.parse::<i64>() {
                    time = Some(Utc.timestamp(t, 0));
                }
            } else {
                // ... ignore paths lines
            }
        }

        // we might have a trailing cmd
        if let Some(cmd) = cmd.take() {
            let time = time.unwrap_or(now);

            loader
                .push(History::new(
                    time,
                    cmd,
                    "unknown".into(),
                    -1,
                    -1,
                    None,
                    None,
                ))
                .await?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod test {

    use crate::import::{tests::TestLoader, Importer};

    use super::Fish;

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
            ($timestamp:expr, $command:expr) => {
                let h = history.next().expect("missing entry in history");
                assert_eq!(h.command.as_str(), $command);
                assert_eq!(h.timestamp.timestamp(), $timestamp);
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
