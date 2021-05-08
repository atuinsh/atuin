use std::{fs::File, path::Path};
use std::{
    io::{BufRead, BufReader},
    path::PathBuf,
};

use directories::UserDirs;
use eyre::{eyre, Result};

use super::{count_lines, Importer};
use crate::history::History;

#[derive(Debug)]
pub struct Bash {
    file: BufReader<File>,
    strbuf: String,
    loc: usize,
    counter: i64,
}

impl Importer for Bash {
    const NAME: &'static str = "bash";

    fn histpath() -> Result<PathBuf> {
        let user_dirs = UserDirs::new().unwrap();
        let home_dir = user_dirs.home_dir();

        Ok(home_dir.join(".bash_history"))
    }

    fn parse(path: impl AsRef<Path>) -> Result<Self> {
        let file = File::open(path)?;
        let mut buf = BufReader::new(file);
        let loc = count_lines(&mut buf)?;

        Ok(Self {
            file: buf,
            strbuf: String::new(),
            loc,
            counter: 0,
        })
    }
}

impl Iterator for Bash {
    type Item = Result<History>;

    fn next(&mut self) -> Option<Self::Item> {
        self.strbuf.clear();
        match self.file.read_line(&mut self.strbuf) {
            Ok(0) => return None,
            Ok(_) => (),
            Err(e) => return Some(Err(eyre!("failed to read line: {}", e))), // we can skip past things like invalid utf8
        }

        self.loc -= 1;

        while self.strbuf.ends_with("\\\n") {
            if self.file.read_line(&mut self.strbuf).is_err() {
                // There's a chance that the last line of a command has invalid
                // characters, the only safe thing to do is break :/
                // usually just invalid utf8 or smth
                // however, we really need to avoid missing history, so it's
                // better to have some items that should have been part of
                // something else, than to miss things. So break.
                break;
            };

            self.loc -= 1;
        }

        let time = chrono::Utc::now();
        let offset = chrono::Duration::seconds(self.counter);
        let time = time - offset;

        self.counter += 1;

        Some(Ok(History::new(
            time,
            self.strbuf.trim_end().to_string(),
            String::from("unknown"),
            -1,
            -1,
            None,
            None,
        )))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (0, Some(self.loc))
    }
}
