// import old shell history!
// automatically hoover up all that we can find

use std::path::PathBuf;

use async_trait::async_trait;
use atuin_common::env;
use atuin_common::path::PathBufExt as _;
use eyre::{Result, eyre};
use jiff::Timestamp;

use super::{Importer, Loader, unix_byte_lines};
use crate::history::History;
use crate::import::read_to_end;

#[derive(Debug)]
pub struct Fish {
    bytes: Vec<u8>,
}

/// see <https://fishshell.com/docs/current/interactive.html#searchable-command-history>
fn default_histpath() -> Result<PathBuf> {
    let data = env::var_os("XDG_DATA_HOME").map(PathBuf::from).ok_or(()).or_else(|_| {
        let Some(home) = dirs::home_dir() else {
            eyre::bail!("could not determine home directory");
        };
        Ok(home.joined(".local").joined("share"))
    })?;

    // fish supports multiple history sessions
    // If `fish_history` var is missing, or set to `default`, use `fish` as the session
    let session = env::var("fish_history").unwrap_or_else(|_| String::from("fish"));
    let session = if session == "default" {
        String::from("fish")
    } else {
        session
    };

    let histpath = data.joined("fish").joined(format!("{session}_history"));

    if histpath.exists() {
        Ok(histpath)
    } else {
        Err(eyre!("Could not find history file."))
    }
}

/// Undo fish's history escaping, where `\\` is a backslash and `\n` is a newline.
fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();

    // Read each escape in one pass, because fish stores a command's literal `\n` as `\\n`.
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }

        match chars.next() {
            Some('\\') => out.push('\\'),
            Some('n') => out.push('\n'),
            // fish writes no other escapes; keep anything else as it is
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }

    out
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
        let now = Timestamp::now();
        let mut time: Option<Timestamp> = None;
        let mut cmd: Option<String> = None;

        let mut process_cmd = async |cmd: &mut Option<String>, time: Option<Timestamp>| {
            let Some(cmd) = cmd.take() else {
                return Ok(());
            };

            let time = time.unwrap_or(now);
            let entry = History::import().shell("fish").timestamp(time).command(cmd);

            loader.push(entry.build().into()).await
        };

        for b in unix_byte_lines(&self.bytes) {
            // we can skip past things like invalid utf8
            let Ok(s) = std::str::from_utf8(b) else {
                continue;
            };

            if let Some(c) = s.strip_prefix("- cmd: ") {
                // first, we must deal with the prev cmd
                process_cmd(&mut cmd, time).await?;

                cmd = Some(unescape(c));
            } else if let Some(t) = s.strip_prefix("  when: ") {
                // ignore the line if it is not an int, or is outside the range we can
                // represent - keeping the previous entry's timestamp preserves ordering,
                // and a corrupt entry must not abort the import
                time = t.parse::<i64>().ok().and_then(|t| Timestamp::from_second(t).ok()).or(time);
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

    use rstest::rstest;

    use super::Fish;
    use crate::import::Importer;
    use crate::import::tests::TestLoader;

    #[rstest]
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
        assert_eq!(loader.buf[0].timestamp.as_second(), 1_639_162_832);
        assert_eq!(loader.buf[1].timestamp.as_second(), 1_639_162_832);
        assert_eq!(loader.buf[2].timestamp.as_second(), 1_639_162_851);
    }

    #[rstest]
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
                assert_eq!(h.timestamp.as_second(), $timestamp);
            };
        }

        fishtory!(1_639_162_832, "history --help");
        fishtory!(1_639_162_851, "cat ~/.bash_history");
        fishtory!(1_639_162_890, "ls ~/.local/share/fish/fish_history");
        fishtory!(1_639_162_893, "cat ~/.local/share/fish/fish_history");
        fishtory!(1_639_162_933, "echo \"foo\" \\\n'bar' baz");
        fishtory!(1_639_162_939, "cat ~/.local/share/fish/fish_history");
        fishtory!(1_639_163_063, r#"echo "\"" \\ "\\""#);
        fishtory!(1_639_163_066, "cat ~/.local/share/fish/fish_history");
    }

    /// fish stores a backslash as `\\` and a newline as `\n`. A literal `\n` in a command is
    /// stored as `\\n` and must not become a newline. Any other backslash is kept.
    #[rstest]
    #[case::printf(r"printf 'a\\nb'", r"printf 'a\nb'")]
    #[case::windows_path(r"cd C:\\new", r"cd C:\new")]
    #[case::two_backslashes(r"echo \\\\n", r"echo \\n")]
    #[case::escaped_newline(r"echo a\nb", "echo a\nb")]
    #[case::unknown_escape(r"echo \t", r"echo \t")]
    #[case::trailing_backslash(r"echo \", r"echo \")]
    #[tokio::test]
    async fn parse_escapes(#[case] stored: &str, #[case] command: &str) {
        let bytes = format!("- cmd: {stored}\n  when: 1639162832\n").into_bytes();

        let fish = Fish { bytes };
        let mut loader = TestLoader::default();
        fish.load(&mut loader).await.unwrap();

        assert_eq!(loader.buf.len(), 1);
        assert_eq!(loader.buf[0].command, command);
    }
}
