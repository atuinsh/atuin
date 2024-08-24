use std::fs::File;
use std::io::Read;
use std::path::PathBuf;

use async_trait::async_trait;
use eyre::{bail, Result};
use memchr::Memchr;

use crate::history::History;

pub mod bash;
pub mod fish;
pub mod nu;
pub mod nu_histdb;
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
    async fn push(&mut self, hist: History) -> eyre::Result<()>;
}

fn unix_byte_lines(input: &[u8]) -> impl DoubleEndedIterator<Item = &[u8]> {
    UnixByteLines {
        iter: memchr::memchr_iter(b'\n', input),
        bytes: input,
        i: 0,
        // Set to the last element
        i_rev: input.len().saturating_sub(1),
    }
}

struct UnixByteLines<'a> {
    iter: Memchr<'a>,
    bytes: &'a [u8],
    // Index for iterating in regular order
    i: usize,
    // Index for iterating in reverse order
    i_rev: usize,
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

impl<'a> DoubleEndedIterator for UnixByteLines<'a> {
    fn next_back(&mut self) -> Option<Self::Item> {
        let needle_idx = match self.iter.next_back() {
            Some(v) => {
                if v == self.i_rev {
                    // The first newline at the very end of the input sequence, skip
                    self.iter.next_back()
                } else {
                    Some(v)
                }
            }
            None => None,
        };
        let range_start = if needle_idx.is_none() && self.i_rev > 0 {
            // Reached the very beginning of the input sequence
            0
        } else {
            // Do not include the found newline in the range
            needle_idx.map(|v| v + 1)?
        };
        let out = &self.bytes[range_start..self.i_rev];
        self.i_rev = needle_idx.unwrap_or(0);
        Some(out)
    }
}

fn count_lines(input: &[u8]) -> usize {
    unix_byte_lines(input).count()
}

fn get_histpath<D>(def: D) -> Result<PathBuf>
where
    D: FnOnce() -> Result<PathBuf>,
{
    if let Ok(p) = std::env::var("HISTFILE") {
        is_file(PathBuf::from(p))
    } else {
        is_file(def()?)
    }
}

fn read_to_end(path: PathBuf) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut f = File::open(path)?;
    f.read_to_end(&mut bytes)?;
    Ok(bytes)
}
fn is_file(p: PathBuf) -> Result<PathBuf> {
    if p.is_file() {
        Ok(p)
    } else {
        bail!("Could not find history file {:?}. Try setting $HISTFILE", p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use itertools::assert_equal;

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

    #[test]
    fn test_double_ended_iterator_unix_byte_lines() {
        let input = "1\n2\n3\n4\n";
        let bytes = unix_byte_lines(input.as_bytes());

        assert_equal(bytes, [b"1", b"2", b"3", b"4"])
    }

    #[test]
    fn test_double_ended_iterator_unix_byte_lines_rev() {
        let input = "1\n2\n3\n4\n";
        let bytes = unix_byte_lines(input.as_bytes());

        assert_equal(bytes.rev(), [b"4", b"3", b"2", b"1"])
    }
}
