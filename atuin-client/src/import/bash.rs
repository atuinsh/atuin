use std::{fs::File, io::Read, path::PathBuf};

use async_trait::async_trait;
use directories::UserDirs;
use eyre::{eyre, Result};

use super::{get_histpath, unix_byte_lines, Importer, Loader};
use crate::history::History;

#[derive(Debug)]
pub struct Bash {
    bytes: Vec<u8>,
}

fn default_histpath() -> Result<PathBuf> {
    let user_dirs = UserDirs::new().ok_or_else(|| eyre!("could not find user directories"))?;
    let home_dir = user_dirs.home_dir();

    Ok(home_dir.join(".bash_history"))
}

#[async_trait]
impl Importer for Bash {
    const NAME: &'static str = "bash";

    async fn new() -> Result<Self> {
        let mut bytes = Vec::new();
        let path = get_histpath(default_histpath)?;
        let mut f = File::open(path)?;
        f.read_to_end(&mut bytes)?;
        Ok(Self { bytes })
    }

    async fn entries(&mut self) -> Result<usize> {
        Ok(super::count_lines(&self.bytes))
    }

    async fn load(self, h: &mut impl Loader) -> Result<()> {
        let now = chrono::Utc::now();
        let mut line = String::new();

        for (i, b) in unix_byte_lines(&self.bytes).enumerate() {
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
