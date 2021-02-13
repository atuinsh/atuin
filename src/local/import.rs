// import old shell history!
// automatically hoover up all that we can find

use std::fs::File;
use std::io::{BufRead, BufReader};

use eyre::Result;

use crate::models::history::History;

pub struct ImportBash {
    file: BufReader<File>,
}

impl ImportBash {
    pub fn new(path: &str) -> Result<ImportBash> {
        let file = File::open(path)?;
        let buf = BufReader::new(file);

        Ok(ImportBash { file: buf })
    }
}

impl Iterator for ImportBash {
    type Item = History;

    fn next(&mut self) -> Option<History> {
        let mut line = String::new();

        match self.file.read_line(&mut line) {
            Ok(0) => None,
            Err(_) => None,

            Ok(_) => Some(History {
                cwd: "none".to_string(),
                command: line,
                timestamp: -1,
            }),
        }
    }
}
