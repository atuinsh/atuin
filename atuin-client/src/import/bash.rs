use std::{
    fs::File,
    io::{self, Read},
    path::PathBuf,
};

use async_trait::async_trait;
use directories::UserDirs;
use eyre::Result;

use super::{Importer, Loader};
use crate::history::History;

#[derive(Debug)]
pub struct Bash {
    bytes: Vec<u8>,
}

fn histpath() -> PathBuf {
    let user_dirs = UserDirs::new().unwrap();
    let home_dir = user_dirs.home_dir();

    home_dir.join(".bash_history")
}

#[async_trait]
impl Importer for Bash {
    const NAME: &'static str = "bash";

    async fn new() -> io::Result<Self> {
        let mut bytes = Vec::new();
        let mut f = File::open(histpath())?;
        f.read_to_end(&mut bytes)?;
        Ok(Self { bytes })
    }

    async fn entries(&mut self) -> Result<usize> {
        Ok(super::count_lines(&self.bytes))
    }

    async fn load(self, h: &mut impl Loader) -> Result<()> {
        let now = chrono::Utc::now();

        let mut i = 0;

        let mut line = String::new();

        for j in memchr::memchr_iter(b'\n', &self.bytes) {
            let b = &self.bytes[i..j];
            i = j + 1; // skip over the \n

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

                let offset = chrono::Duration::seconds(i as i64);
                h.push(History::new(
                    now - offset, // preserve ordering
                    command,
                    String::from("unknown"),
                    -1,
                    -1,
                    None,
                    None,
                ))
                .await?;
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use itertools::assert_equal;

    use crate::import::{tests::TestLoader, Importer};

    use super::Bash;

    #[tokio::test]
    async fn test_parse_file() {
        let bytes = r"cargo install atuin
cargo install atuin; \
cargo update
cargo :b̷i̶t̴r̵o̴t̴ ̵i̷s̴ ̷r̶e̵a̸l̷
"
        .as_bytes()
        .to_owned();

        let mut bash = Bash { bytes };
        assert_eq!(bash.entries().await.unwrap(), 4);

        let mut loader = TestLoader::default();
        bash.load(&mut loader).await.unwrap();

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
