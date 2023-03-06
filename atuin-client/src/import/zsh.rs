// import old shell history!
// automatically hoover up all that we can find

use std::{
    fs,
    path::{Path, PathBuf},
};

use async_trait::async_trait;
use chrono::{prelude::*, Utc};
use directories::UserDirs;
use eyre::{bail, Result};

use super::{unix_byte_lines, Importer, Loader};
use crate::history::History;

#[derive(Debug)]
pub struct Zsh {
    bytes: Vec<u8>,
}

#[async_trait]
impl Importer for Zsh {
    const NAME: &'static str = "zsh";

    fn default_source_path() -> Result<PathBuf> {
        let Some(user_dirs) = UserDirs::new() else {
            bail!("could not find user directories");
        };
        let home_dir = user_dirs.home_dir();

        // oh-my-zsh sets HISTFILE=~/.zhistory
        // zsh has no default value for this var, but uses ~/.zhistory.
        // we could maybe be smarter about this in the future :)
        let Some(path) = [".zhistory", ".zsh_history"].iter().find_map(|f| {
            let path = home_dir.join(f);
            path.exists().then_some(path)
        }) else {
            bail!("Found neither .zhistory nor .zsh_history");
        };

        Ok(path)
    }

    async fn new(source: &Path) -> Result<Self> {
        let bytes = fs::read(source)?;
        Ok(Self { bytes })
    }

    async fn entries(&mut self) -> Result<usize> {
        Ok(super::count_lines(&self.bytes))
    }

    async fn load(self, h: &mut impl Loader) -> Result<()> {
        let now = chrono::Utc::now();
        let mut line = String::new();

        let mut counter = 0;
        for b in unix_byte_lines(&self.bytes) {
            let s = match std::str::from_utf8(b) {
                Ok(s) => s,
                Err(_) => continue, // we can skip past things like invalid utf8
            };

            if let Some(s) = s.strip_suffix('\\') {
                line.push_str(s);
                line.push_str("\\\n");
            } else {
                line.push_str(s);
                let command = std::mem::take(&mut line);

                if let Some(command) = command.strip_prefix(": ") {
                    counter += 1;
                    h.push(parse_extended(command, counter)).await?;
                } else {
                    let offset = chrono::Duration::seconds(counter);
                    counter += 1;

                    h.push(History::new(
                        now - offset, // preserve ordering
                        command.trim_end().to_string(),
                        String::from("unknown"),
                        -1,
                        -1,
                        None,
                        None,
                    ))
                    .await?;
                }
            }
        }

        Ok(())
    }
}

fn parse_extended(line: &str, counter: i64) -> History {
    let (time, duration) = line.split_once(':').unwrap();
    let (duration, command) = duration.split_once(';').unwrap();

    let time = time
        .parse::<i64>()
        .unwrap_or_else(|_| chrono::Utc::now().timestamp());

    let offset = chrono::Duration::milliseconds(counter);
    let time = Utc.timestamp(time, 0);
    let time = time + offset;

    let duration = duration.parse::<i64>().map_or(-1, |t| t * 1_000_000_000);

    // use nanos, because why the hell not? we won't display them.
    History::new(
        time,
        command.trim_end().to_string(),
        String::from("unknown"),
        0, // assume 0, we have no way of knowing :(
        duration,
        None,
        None,
    )
}

#[cfg(test)]
mod test {
    use chrono::prelude::*;
    use chrono::Utc;
    use itertools::assert_equal;

    use crate::import::tests::TestLoader;

    use super::*;

    #[test]
    fn test_parse_extended_simple() {
        let parsed = parse_extended("1613322469:0;cargo install atuin", 0);

        assert_eq!(parsed.command, "cargo install atuin");
        assert_eq!(parsed.duration, 0);
        assert_eq!(parsed.timestamp, Utc.timestamp(1_613_322_469, 0));

        let parsed = parse_extended("1613322469:10;cargo install atuin;cargo update", 0);

        assert_eq!(parsed.command, "cargo install atuin;cargo update");
        assert_eq!(parsed.duration, 10_000_000_000);
        assert_eq!(parsed.timestamp, Utc.timestamp(1_613_322_469, 0));

        let parsed = parse_extended("1613322469:10;cargo :b̷i̶t̴r̵o̴t̴ ̵i̷s̴ ̷r̶e̵a̸l̷", 0);

        assert_eq!(parsed.command, "cargo :b̷i̶t̴r̵o̴t̴ ̵i̷s̴ ̷r̶e̵a̸l̷");
        assert_eq!(parsed.duration, 10_000_000_000);
        assert_eq!(parsed.timestamp, Utc.timestamp(1_613_322_469, 0));

        let parsed = parse_extended("1613322469:10;cargo install \\n atuin\n", 0);

        assert_eq!(parsed.command, "cargo install \\n atuin");
        assert_eq!(parsed.duration, 10_000_000_000);
        assert_eq!(parsed.timestamp, Utc.timestamp(1_613_322_469, 0));
    }

    #[tokio::test]
    async fn test_parse_file() {
        let bytes = r": 1613322469:0;cargo install atuin
: 1613322469:10;cargo install atuin; \
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
}
