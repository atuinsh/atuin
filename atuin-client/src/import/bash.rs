use std::io::{BufRead, BufReader};
use std::{fs::File, path::Path};

use eyre::{eyre, Result};

use super::count_lines;
use crate::history::History;

#[derive(Debug)]
pub struct Bash {
    file: BufReader<File>,

    pub loc: u64,
    pub counter: i64,
}

impl Bash {
    pub fn new(path: impl AsRef<Path>) -> Result<Self> {
        let file = File::open(path)?;
        let mut buf = BufReader::new(file);
        let loc = count_lines(&mut buf)?;

        Ok(Self {
            file: buf,
            loc: loc as u64,
            counter: 0,
        })
    }

    fn read_line(&mut self) -> Option<Result<String>> {
        let mut line = String::new();

        match self.file.read_line(&mut line) {
            Ok(0) => None,
            Ok(_) => Some(Ok(line)),
            Err(e) => Some(Err(eyre!("failed to read line: {}", e))), // we can skip past things like invalid utf8
        }
    }
}

impl Iterator for Bash {
    type Item = Result<History>;

    fn next(&mut self) -> Option<Self::Item> {
        let line = self.read_line()?;

        if let Err(e) = line {
            return Some(Err(e)); // :(
        }

        let mut line = line.unwrap();

        while line.ends_with("\\\n") {
            let next_line = self.read_line()?;

            if next_line.is_err() {
                break;
            }

            line.push_str(next_line.unwrap().as_str());
        }

        let time = chrono::Utc::now();
        let offset = chrono::Duration::seconds(self.counter);
        let time = time - offset;

        self.counter += 1;

        Some(Ok(History::new(
            time,
            line.trim_end().to_string(),
            String::from("unknown"),
            -1,
            -1,
            None,
            None,
        )))
    }
}
