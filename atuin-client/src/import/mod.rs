use std::{
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

use eyre::Result;

use crate::history::History;

pub mod bash;
pub mod fish;
pub mod resh;
pub mod zsh;
pub mod zsh_histdb;

// this could probably be sped up
fn count_lines(buf: &mut BufReader<impl Read + Seek>) -> Result<usize> {
    let lines = buf.lines().count();
    buf.seek(SeekFrom::Start(0))?;

    Ok(lines)
}

pub trait Importer: IntoIterator<Item = Result<History>> + Sized {
    const NAME: &'static str;
    fn histpath() -> Result<PathBuf>;
    fn parse(path: impl AsRef<Path>) -> Result<Self>;
}
