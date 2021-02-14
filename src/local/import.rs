// import old shell history!
// automatically hoover up all that we can find

use std::fs::File;
use std::io::{BufRead, BufReader};

use eyre::{eyre, Result};

use crate::local::history::History;

#[derive(Debug)]
pub struct Zsh {
    file: BufReader<File>,

    pub loc: u64,
}

// this could probably be sped up
fn count_lines(path: &str) -> Result<usize> {
    let file = File::open(path)?;
    let buf = BufReader::new(file);

    Ok(buf.lines().count())
}

impl Zsh {
    pub fn new(path: &str) -> Result<Self> {
        let loc = count_lines(path)?;

        let file = File::open(path)?;
        let buf = BufReader::new(file);

        Ok(Self {
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

fn parse_extended(line: &str) -> History {
    let line = line.replacen(": ", "", 2);
    let mut split = line.splitn(2, ':');

    let time = split.next().unwrap_or("-1");
    let time = time
        .parse::<i64>()
        .unwrap_or_else(|_| chrono::Utc::now().timestamp_nanos());

    let duration_command = split.next().unwrap(); // might be 0;the command
    let mut split = duration_command.split(';');

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

impl Iterator for Zsh {
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
                let extended = line.starts_with(':');

                if extended {
                    Some(Ok(parse_extended(line.as_str())))
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
