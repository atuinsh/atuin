use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::{
    fs::File,
    path::{Path, PathBuf},
};

use eyre::Result;

use crate::history::History;

pub mod bash;
pub mod resh;
pub mod zsh;

// this could probably be sped up
fn count_lines(buf: &mut BufReader<File>) -> Result<usize> {
    let lines = buf.lines().count();
    buf.seek(SeekFrom::Start(0))?;

    Ok(lines)
}

pub trait Importer: IntoIterator<Item = Result<History>> + Sized {
    fn histpath() -> Result<PathBuf>;
    fn parse(path: impl AsRef<Path>) -> Result<Self>;
    fn count(&self) -> u64;
}
