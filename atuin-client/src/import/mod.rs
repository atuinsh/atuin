use std::path::{Path, PathBuf};

use async_trait::async_trait;
use eyre::{bail, Result};
use memchr::Memchr;

use crate::history::History;

pub mod bash;
pub mod fish;
pub mod resh;
pub mod zsh;
pub mod zsh_histdb;

#[async_trait]
pub trait Importer: Sized {
    const NAME: &'static str;
    fn default_source_path() -> Result<PathBuf>;
    fn final_source_path(
        cli_custom_source: Option<&Path>,
        env_custom_source: Option<&Path>,
    ) -> Result<PathBuf> {
        let candidate = 'candidate: {
            // CLI has highest precedence
            if let Some(p) = cli_custom_source {
                break 'candidate p.to_owned();
            }
            // Env var has second highest precedence
            if let Some(p) = env_custom_source {
                break 'candidate p.to_owned();
            }
            // Default has lowest precedence
            Self::default_source_path()?
        };

        if points_to_file(&candidate) {
            Ok(candidate)
        } else {
            bail!("{candidate:?} is neither a file nor a symlink to a file.");
        }
    }
    /// `source` passed to this function is guaranteed to be an existing file.
    async fn new(source: &Path) -> Result<Self>;
    async fn entries(&mut self) -> Result<usize>;
    async fn load(self, loader: &mut impl Loader) -> Result<()>;
}

/// Gets whether a path is a file, or is a symlink that eventually points to a file.
pub fn points_to_file(path: impl AsRef<Path>) -> bool {
    path.as_ref()
        .canonicalize()
        .map(|p| p.is_file())
        .unwrap_or(false)
}

#[async_trait]
pub trait Loader: Sync + Send {
    async fn push(&mut self, hist: History) -> eyre::Result<()>;
}

fn unix_byte_lines(input: &[u8]) -> impl Iterator<Item = &[u8]> {
    UnixByteLines {
        iter: memchr::memchr_iter(b'\n', input),
        bytes: input,
        i: 0,
    }
}

struct UnixByteLines<'a> {
    iter: Memchr<'a>,
    bytes: &'a [u8],
    i: usize,
}

impl<'a> Iterator for UnixByteLines<'a> {
    type Item = &'a [u8];

    fn next(&mut self) -> Option<Self::Item> {
        let j = self.iter.next()?;
        let out = &self.bytes[self.i..j];
        self.i = j + 1;
        Some(out)
    }

    fn count(self) -> usize
    where
        Self: Sized,
    {
        self.iter.count()
    }
}

fn count_lines(input: &[u8]) -> usize {
    unix_byte_lines(input).count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    pub struct TestLoader {
        pub buf: Vec<History>,
    }

    #[async_trait]
    impl Loader for TestLoader {
        async fn push(&mut self, hist: History) -> Result<()> {
            self.buf.push(hist);
            Ok(())
        }
    }
}
