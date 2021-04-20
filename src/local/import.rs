// import old shell history!
// automatically hoover up all that we can find

use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::{fs::File, path::Path};

use chrono::prelude::*;
use chrono::Utc;
use eyre::{eyre, Result};
use itertools::Itertools;

use super::history::History;

#[derive(Debug)]
pub struct Zsh {
    file: BufReader<File>,

    pub loc: u64,
    pub counter: i64,
}

// this could probably be sped up
fn count_lines(buf: &mut BufReader<File>) -> Result<usize> {
    let lines = buf.lines().count();
    buf.seek(SeekFrom::Start(0))?;

    Ok(lines)
}

impl Zsh {
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
}

fn parse_extended(line: &str, counter: i64) -> History {
    let line = line.replacen(": ", "", 2);
    let (time, duration) = line.splitn(2, ':').collect_tuple().unwrap();
    let (duration, command) = duration.splitn(2, ';').collect_tuple().unwrap();

    let time = time
        .parse::<i64>()
        .unwrap_or_else(|_| chrono::Utc::now().timestamp());

    let offset = chrono::Duration::milliseconds(counter);
    let time = Utc.timestamp(time, 0);
    let time = time + offset;

    let duration = duration.parse::<i64>().map_or(-1, |t| t * 1_000_000_000);

    // use nanos, because why the hell not? we won't display them.
    History::new(
        time,
        command.trim_end().to_string(),
        String::from("unknown"),
        0, // assume 0, we have no way of knowing :(
        duration,
        None,
        None,
    )
}

impl Zsh {
    fn read_line(&mut self) -> Option<Result<String>> {
        let mut line = String::new();

        match self.file.read_line(&mut line) {
            Ok(0) => None,
            Ok(_) => Some(Ok(line)),
            Err(e) => Some(Err(eyre!("failed to read line: {}", e))), // we can skip past things like invalid utf8
        }
    }
}

impl Iterator for Zsh {
    type Item = Result<History>;

    fn next(&mut self) -> Option<Self::Item> {
        // ZSH extended history records the timestamp + command duration
        // These lines begin with :
        // So, if the line begins with :, parse it. Otherwise it's just
        // the command
        let line = self.read_line()?;

        if let Err(e) = line {
            return Some(Err(e)); // :(
        }

        let mut line = line.unwrap();

        while line.ends_with("\\\n") {
            let next_line = self.read_line()?;

            if next_line.is_err() {
                // There's a chance that the last line of a command has invalid
                // characters, the only safe thing to do is break :/
                // usually just invalid utf8 or smth
                // however, we really need to avoid missing history, so it's
                // better to have some items that should have been part of
                // something else, than to miss things. So break.
                break;
            }

            line.push_str(next_line.unwrap().as_str());
        }

        // We have to handle the case where a line has escaped newlines.
        // Keep reading until we have a non-escaped newline

        let extended = line.starts_with(':');

        if extended {
            self.counter += 1;
            Some(Ok(parse_extended(line.as_str(), self.counter)))
        } else {
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
}

#[cfg(test)]
mod test {
    use chrono::prelude::*;
    use chrono::Utc;

    use super::parse_extended;

    #[test]
    fn test_parse_extended_simple() {
        let parsed = parse_extended(": 1613322469:0;cargo install atuin", 0);

        assert_eq!(parsed.command, "cargo install atuin");
        assert_eq!(parsed.duration, 0);
        assert_eq!(parsed.timestamp, Utc.timestamp(1_613_322_469, 0));

        let parsed = parse_extended(": 1613322469:10;cargo install atuin;cargo update", 0);

        assert_eq!(parsed.command, "cargo install atuin;cargo update");
        assert_eq!(parsed.duration, 10_000_000_000);
        assert_eq!(parsed.timestamp, Utc.timestamp(1_613_322_469, 0));

        let parsed = parse_extended(": 1613322469:10;cargo :b̷i̶t̴r̵o̴t̴ ̵i̷s̴ ̷r̶e̵a̸l̷", 0);

        assert_eq!(parsed.command, "cargo :b̷i̶t̴r̵o̴t̴ ̵i̷s̴ ̷r̶e̵a̸l̷");
        assert_eq!(parsed.duration, 10_000_000_000);
        assert_eq!(parsed.timestamp, Utc.timestamp(1_613_322_469, 0));

        let parsed = parse_extended(": 1613322469:10;cargo install \\n atuin\n", 0);

        assert_eq!(parsed.command, "cargo install \\n atuin");
        assert_eq!(parsed.duration, 10_000_000_000);
        assert_eq!(parsed.timestamp, Utc.timestamp(1_613_322_469, 0));
    }
}
