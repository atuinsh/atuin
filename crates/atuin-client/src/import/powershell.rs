use async_trait::async_trait;
use directories::BaseDirs;
use eyre::{Result, eyre};
use std::path::PathBuf;
use time::{Duration, OffsetDateTime};

use super::{Importer, Loader, count_lines, unix_byte_lines};
use crate::history::History;
use crate::import::read_to_end;

#[derive(Debug)]
pub struct PowerShell {
    bytes: Vec<u8>,
    line_count: Option<usize>,
}

fn get_history_path() -> Result<PathBuf> {
    let base = BaseDirs::new().ok_or_else(|| eyre!("could not determine data directory"))?;

    // The command line history in PowerShell is maintained by the PSReadLine module:
    // https://learn.microsoft.com/en-us/powershell/module/psreadline/about/about_psreadline#command-history
    //
    // > PSReadLine maintains a history file containing all the commands and data you've entered from the command line.
    // > The history files are a file named `$($Host.Name)_history.txt`.
    // > On Windows systems the history file is stored at `$Env:APPDATA\Microsoft\Windows\PowerShell\PSReadLine`.
    // > On non-Windows systems, the history files are stored at `$Env:XDG_DATA_HOME/powershell/PSReadLine`
    // > or `$Env:HOME/.local/share/powershell/PSReadLine`.

    let dir = if cfg!(windows) {
        base.data_dir()
            .join("Microsoft")
            .join("Windows")
            .join("PowerShell")
            .join("PSReadLine")
    } else {
        std::env::var("XDG_DATA_HOME")
            .map_or_else(
                |_| base.home_dir().join(".local").join("share"),
                PathBuf::from,
            )
            .join("powershell")
            .join("PSReadLine")
    };

    // The history is stored in a file named `$($Host.Name)_history.txt`.
    // For the default console host shipped by Microsoft,`$Host.Name` is `ConsoleHost`:
    // https://learn.microsoft.com/en-us/dotnet/api/system.management.automation.host.pshost.name#remarks

    let file = dir.join("ConsoleHost_history.txt");

    if file.is_file() {
        Ok(file)
    } else {
        Err(eyre!("Could not find history file: {}", file.display()))
    }
}

#[async_trait]
impl Importer for PowerShell {
    const NAME: &'static str = "PowerShell";

    async fn new() -> Result<Self> {
        let bytes = read_to_end(get_history_path()?)?;
        Ok(Self {
            bytes,
            line_count: None,
        })
    }

    async fn entries(&mut self) -> Result<usize> {
        // Commands can be split over multiple lines,
        // but this is only used for a progress bar, and multi-line commands
        // should be quite rare, so this is not an issue in practice.
        if self.line_count.is_none() {
            self.line_count = Some(count_lines(&self.bytes));
        }
        Ok(self.line_count.unwrap())
    }

    async fn load(mut self, h: &mut impl Loader) -> Result<()> {
        let line_count = self.entries().await?;
        let start = OffsetDateTime::now_utc() - Duration::milliseconds(line_count as i64);

        let mut counter = 0;
        let mut iter = unix_byte_lines(&self.bytes);

        while let Some(s) = iter.next() {
            let Ok(s) = read_line(s) else {
                continue; // We can skip past things like invalid utf8
            };

            let mut cmd = s.to_string();

            // Multi-line commands end with a backtick, append the following lines.
            while cmd.ends_with('`') {
                cmd.pop();

                let Some(next) = iter.next() else {
                    break;
                };
                let Ok(next) = read_line(next) else {
                    break;
                };

                cmd.push('\n');
                cmd.push_str(next);
            }

            if cmd.is_empty() {
                continue;
            }

            let offset = Duration::milliseconds(counter);
            counter += 1;

            let entry = History::import().timestamp(start + offset).command(cmd);
            h.push(entry.build().into()).await?;
        }

        Ok(())
    }
}

fn read_line(s: &[u8]) -> Result<&str> {
    let s = str::from_utf8(s)?;

    // History is stored in CRLF on Windows, normalize the input to LF on all platforms.
    let s = s.strip_suffix('\r').unwrap_or(s);

    Ok(s)
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::import::tests::TestLoader;
    use itertools::assert_equal;

    const INPUT: &str = r#"cargo install atuin
cargo update
echo "first line`
second line`
`
last line"
echo foo

echo bar
echo baz
"#;

    const EXPECTED: &[&str] = &[
        "cargo install atuin",
        "cargo update",
        "echo \"first line\nsecond line\n\nlast line\"",
        "echo foo",
        "echo bar",
        "echo baz",
    ];

    #[tokio::test]
    async fn test_import() {
        let loader = import(INPUT).await;

        let actual = loader.buf.iter().map(|h| h.command.clone());
        let expected = EXPECTED.iter().map(|s| s.to_string());

        assert_equal(actual, expected);
    }

    #[tokio::test]
    async fn test_crlf() {
        let input = INPUT.replace("\n", "\r\n");
        let loader = import(input.as_str()).await;

        let actual = loader.buf.iter().map(|h| h.command.clone());
        let expected = EXPECTED.iter().map(|s| s.to_string());

        assert_equal(actual, expected);
    }

    #[tokio::test]
    async fn test_timestamps() {
        let loader = import(INPUT).await;

        let mut prev = loader.buf.first().unwrap().timestamp;
        for current in loader.buf.iter().skip(1).map(|h| h.timestamp) {
            assert!(current > prev);
            prev = current;
        }
    }

    async fn import(input: &str) -> TestLoader {
        let powershell = PowerShell {
            bytes: input.as_bytes().to_vec(),
            line_count: None,
        };

        let mut loader = TestLoader::default();
        powershell.load(&mut loader).await.unwrap();
        loader
    }
}
