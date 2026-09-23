use std::path::PathBuf;

use async_trait::async_trait;
use atuin_common::fs;
use eyre::{Result, bail};
use memchr::Memchr;

use crate::history::History;

pub mod bash;
pub mod fish;
pub mod nu;
pub mod nu_histdb;
pub mod powershell;
pub mod replxx;
pub mod resh;
pub mod xonsh;
pub mod xonsh_sqlite;
pub mod zsh;
pub mod zsh_histdb;

#[async_trait]
pub trait Importer: Sized {
    const NAME: &'static str;
    async fn new() -> Result<Self>;
    async fn entries(&mut self) -> Result<usize>;
    async fn load(self, loader: &mut impl Loader) -> Result<()>;
}

#[async_trait]
pub trait Loader: Sync + Send {
    async fn push(&mut self, hist: History) -> Result<()>;
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

async fn get_histpath(def: impl Future<Output = Result<PathBuf>>) -> Result<PathBuf> {
    if let Ok(p) = std::env::var("HISTFILE") {
        Ok(PathBuf::from(p))
    } else {
        def.await
    }
}

async fn get_histfile_path(def: impl Future<Output = Result<PathBuf>>) -> Result<PathBuf> {
    is_file(get_histpath(def).await?).await
}

async fn get_histdir_path(def: impl Future<Output = Result<PathBuf>>) -> Result<PathBuf> {
    is_dir(get_histpath(def).await?).await
}

async fn read_to_end(path: PathBuf) -> Result<Vec<u8>> {
    Ok(fs::read(path).await?)
}
async fn is_file(p: PathBuf) -> Result<PathBuf> {
    if fs::metadata(&p).await.is_ok_and(|m| m.is_file()) {
        Ok(p)
    } else {
        bail!("Could not find history file {:?}. Try setting and exporting $HISTFILE", p);
    }
}
async fn is_dir(p: PathBuf) -> Result<PathBuf> {
    if fs::metadata(&p).await.is_ok_and(|m| m.is_dir()) {
        Ok(p)
    } else {
        bail!("Could not find history directory {:?}. Try setting and exporting $HISTFILE", p);
    }
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
