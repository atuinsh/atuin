// import old shell history!
// automatically hoover up all that we can find

use std::fs::File;
use std::io::{BufRead, BufReader};

use chrono::{TimeZone, Utc};
use eyre::{eyre, Result};

use crate::local::history::History;

#[derive(Debug)]
pub struct ImportZsh {
    file: BufReader<File>,

    pub loc: u64,
}

// this could probably be sped up
fn count_lines(path: &str) -> Result<usize> {
    let file = File::open(path)?;
    let buf = BufReader::new(file);

    Ok(buf.lines().count())
}

impl ImportZsh {
    pub fn new(path: &str) -> Result<ImportZsh> {
        let loc = count_lines(path)?;

        let file = File::open(path)?;
        let buf = BufReader::new(file);

        Ok(ImportZsh {
            file: buf,
            loc: loc as u64,
        })
    }
}

fn trim_newline(s: &str) -> String {
    let mut s = String::from(s);

    if s.ends_with('\n') {
        s.pop();
        if s.ends_with('\r') {
            s.pop();
        }
    }

    s
}

fn parse_extended(line: String) -> History {
    let line = line.replacen(": ", "", 2);
    let mut split = line.splitn(2, ":");

    let time = split.next().unwrap_or("-1");
    let time = time
        .parse::<i64>()
        .unwrap_or(chrono::Utc::now().timestamp_nanos());

    let duration = split.next().unwrap(); // might be 0;the command
    let mut split = duration.split(";");

    let duration = split.next().unwrap_or("-1"); // should just be the 0
    let duration = duration.parse::<i64>().unwrap_or(-1);

    let command = split.next().unwrap();

    // use nanos, because why the hell not? we won't display them.
    History::new(
        time * 1_000_000_000,
        trim_newline(command),
        String::from("unknown"),
        -1,
        duration * 1_000_000_000,
        None,
        None,
    )
}

impl Iterator for ImportZsh {
    type Item = Result<History>;

    fn next(&mut self) -> Option<Self::Item> {
        // ZSH extended history records the timestamp + command duration
        // These lines begin with :
        // So, if the line begins with :, parse it. Otherwise it's just
        // the command
        let mut line = String::new();

        match self.file.read_line(&mut line) {
            Ok(0) => None,
            Err(e) => Some(Err(eyre!("failed to parse line: {}", e))),

            Ok(_) => {
                let extended = line.starts_with(":");

                if extended {
                    Some(Ok(parse_extended(line)))
                } else {
                    Some(Ok(History::new(
                        chrono::Utc::now().timestamp_nanos(), // what else? :/
                        trim_newline(line.as_str()),
                        String::from("unknown"),
                        -1,
                        -1,
                        None,
                        None,
                    )))
                }
            }
        }
    }
}
