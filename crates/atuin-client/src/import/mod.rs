use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::path::PathBuf;

use async_trait::async_trait;
use atuin_common::uuid::UuidV7Ext;
use eyre::{Result, bail};
use memchr::Memchr;
use time::OffsetDateTime;
use uuid::Uuid;

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

/// The sessions of history imported from another tool, each a UUIDv7 that importing the same
/// history again reproduces.
///
/// An id is the name-based UUIDv7 of the tool's own key for the session within the importer's
/// namespace, stamped with the session's first entry, so an importer asks for ids in the order it
/// reads its entries.
pub(crate) struct ImportedSessions {
    importer: &'static str,
    ids: HashMap<String, Uuid>,
}

impl ImportedSessions {
    pub(crate) fn new(importer: &'static str) -> Self {
        Self {
            importer,
            ids: HashMap::new(),
        }
    }

    /// The id of the tool's session `key`, stamped `first_seen` when this is its first entry.
    pub(crate) fn id(&mut self, key: &str, first_seen: OffsetDateTime) -> String {
        let importer = self.importer;
        self.ids
            .entry(key.to_owned())
            .or_insert_with(|| Uuid::new_v7_named(first_seen, importer, key.as_bytes()))
            .as_simple()
            .to_string()
    }
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
        bail!("Could not find history file {:?}. Try setting and exporting $HISTFILE", p);
    }
}
fn is_dir(p: PathBuf) -> Result<PathBuf> {
    if p.is_dir() {
        Ok(p)
    } else {
        bail!("Could not find history directory {:?}. Try setting and exporting $HISTFILE", p);
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;
    use time::macros::datetime;

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

    #[rstest]
    fn imported_sessions_name_each_source_session_once_and_again_on_reimport() {
        let start = datetime!(2024-01-02 03:04:05 UTC);
        let mut sessions = ImportedSessions::new("nu_histdb");
        let first = sessions.id("1", start);

        assert_eq!(sessions.id("1", datetime!(2024-01-02 04:00 UTC)), first);
        assert_ne!(sessions.id("2", start), first);
        assert_eq!(ImportedSessions::new("nu_histdb").id("1", start), first);
        assert_ne!(ImportedSessions::new("zsh_histdb").id("1", start), first);

        let id = Uuid::parse_str(&first).unwrap();
        assert_eq!(id.as_simple().to_string(), first);
        assert_eq!(id.get_version(), Some(uuid::Version::SortRand));
        assert_eq!(id.get_timestamp().unwrap().to_unix(), (1_704_164_645, 0));
    }
}
