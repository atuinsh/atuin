use std::fs::File;
use std::io::Read;
use std::path::PathBuf;

use async_trait::async_trait;
use eyre::{Result, bail};
use memchr::Memchr;
use time::{Duration, OffsetDateTime};

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

/// Build an [`OffsetDateTime`] from whole seconds since the unix epoch plus a
/// nanosecond offset.
///
/// Returns `None` rather than panicking when the value falls outside the range
/// `time` can represent. History files and databases are routinely corrupted, and
/// a single bad entry must never abort an import - see
/// <https://github.com/atuinsh/atuin/issues/938>.
#[allow(dead_code)]
fn timestamp_from_parts(secs: i64, nanos: i64) -> Option<OffsetDateTime> {
    OffsetDateTime::from_unix_timestamp(secs)
        .ok()?
        .checked_add(Duration::nanoseconds(nanos))
}

fn get_histpath<D>(def: D) -> Result<PathBuf>
where
    D: FnOnce() -> Result<PathBuf>,
{
    if let Ok(p) = std::env::var("HISTFILE") {
        Ok(PathBuf::from(p))
    } else {
        def()
    }
}

fn get_histfile_path<D>(def: D) -> Result<PathBuf>
where
    D: FnOnce() -> Result<PathBuf>,
{
    get_histpath(def).and_then(is_file)
}

fn get_histdir_path<D>(def: D) -> Result<PathBuf>
where
    D: FnOnce() -> Result<PathBuf>,
{
    get_histpath(def).and_then(is_dir)
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
        bail!(
            "Could not find history file {:?}. Try setting and exporting $HISTFILE",
            p
        );
    }
}
fn is_dir(p: PathBuf) -> Result<PathBuf> {
    if p.is_dir() {
        Ok(p)
    } else {
        bail!(
            "Could not find history directory {:?}. Try setting and exporting $HISTFILE",
            p
        );
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

    #[test]
    fn timestamp_from_parts_combines_secs_and_nanos() {
        let t = timestamp_from_parts(1_639_162_832, 500_000_000).expect("in range");
        assert_eq!(t.unix_timestamp(), 1_639_162_832);
        assert_eq!(t.nanosecond(), 500_000_000);
    }

    #[test]
    fn timestamp_from_parts_rejects_out_of_range_secs() {
        // the value that killed the import in #938
        assert_eq!(timestamp_from_parts(999_999_999_999_999, 0), None);
        assert_eq!(timestamp_from_parts(i64::MIN, 0), None);
        assert_eq!(timestamp_from_parts(i64::MAX, 0), None);
    }

    #[test]
    fn timestamp_from_parts_rejects_overflowing_nanos() {
        // 9999-12-31T23:59:59Z, the last second `time` can represent
        const MAX_SECS: i64 = 253_402_300_799;
        assert!(
            timestamp_from_parts(MAX_SECS, 0).is_some(),
            "precondition: MAX_SECS must itself be representable"
        );
        // adding ~292 years of nanoseconds must not panic
        assert_eq!(timestamp_from_parts(MAX_SECS, i64::MAX), None);
    }
}
