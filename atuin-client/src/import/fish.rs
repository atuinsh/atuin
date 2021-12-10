// import old shell history!
// automatically hoover up all that we can find

use std::{
    fs::File,
    io::{self, BufRead, BufReader, Read, Seek},
    path::{Path, PathBuf},
};

use chrono::prelude::*;
use chrono::Utc;
use directories::UserDirs;
use eyre::{eyre, Result};
use itertools::Itertools;

use super::{count_lines, Importer};
use crate::history::History;

#[derive(Debug)]
pub struct Fish<R> {
    file: BufReader<R>,
    strbuf: String,
    loc: usize,
}

impl<R: Read + Seek> Fish<R> {
    fn new(r: R) -> Result<Self> {
        let mut buf = BufReader::new(r);
        let loc = count_lines(&mut buf)?;

        Ok(Self {
            file: buf,
            strbuf: String::new(),
            loc,
        })
    }

    fn new_entry(&mut self) -> io::Result<bool> {
        let inner = self.file.fill_buf()?;
        Ok(inner.starts_with(b"- "))
    }
}

impl Importer for Fish<File> {
    const NAME: &'static str = "fish";

    fn histpath() -> Result<PathBuf> {
        let user_dirs = UserDirs::new().unwrap();
        let home_dir = user_dirs.home_dir();

        let histpath = home_dir.join(".local/share/fish/fish_history");
        if histpath.exists() {
            Ok(histpath)
        } else {
            Err(eyre!("Could not find history file. Try setting $HISTFILE"))
        }
    }

    fn parse(path: impl AsRef<Path>) -> Result<Self> {
        Self::new(File::open(path)?)
    }
}

impl<R: Read> Iterator for Fish<R> {
    type Item = Result<History>;

    fn next(&mut self) -> Option<Self::Item> {
        let mut time: Option<DateTime<Utc>> = None;
        let mut cmd: Option<String> = None;

        loop {
            self.strbuf.clear();
            match self.file.read_line(&mut self.strbuf) {
                // no more content to read
                Ok(0) => break,
                // bail on IO error
                e @ Err(_) => Some(e),
                _ => (),
            }

            // `read_line` adds the line delimeter to the string. No thanks
            self.strbuf.pop();

            if let Some(c) = self.strbuf.strip_prefix("- cmd: ") {
                // using raw strings to avoid needing escaping.
                // replaces double backslashes with single backslashes
                let c = c.replace(r"\\", r"\");
                // replaces escaped newlines
                let c = c.replace(r"\n", "\n");
                // TODO: any other escape characters?

                cmd = Some(c);
            } else if let Some(t) = self.strbuf.strip_prefix("  when: ") {
                // if t is not an int, just ignore this line
                if let Ok(t) = t.parse::<i64>() {
                    time = Some(Utc.timestamp(t, 0));
                }
            } else {
                // ... ignore paths lines
            }

            if self.new_entry() {
                // next line is a new entry, so let's stop here
                break;
            }
        }

        let cmd = cmd?;
        let time = time.unwrap_or_else(|| Utc::now());

        Some(Ok(History::new(
            time,
            cmd,
            "unknown".into(),
            -1,
            -1,
            None,
            None,
        )))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        // worst case, entry per line
        (0, Some(self.loc))
    }
}

#[cfg(test)]
mod test {}
