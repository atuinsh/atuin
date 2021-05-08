use std::fs::File;
use std::io::{BufRead, BufReader, Seek, SeekFrom};

use eyre::Result;

pub mod bash;
pub mod resh;
pub mod zsh;

// this could probably be sped up
fn count_lines(buf: &mut BufReader<File>) -> Result<usize> {
    let lines = buf.lines().count();
    buf.seek(SeekFrom::Start(0))?;

    Ok(lines)
}
