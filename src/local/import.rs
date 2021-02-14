// import old shell history!
// automatically hoover up all that we can find

use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::{fs::File, path::Path};

use eyre::{Result, WrapErr};

use super::history::History;

#[derive(Debug)]
pub struct Zsh {
    file: BufReader<File>,

    pub loc: u64,
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
        })
    }
}

fn parse_extended(line: &str) -> History {
    let line = line.replacen(": ", "", 2);
    let (time, duration) = line.split_once(':').unwrap();
    let (duration, command) = duration.split_once(';').unwrap();

    let time = time.parse::<i64>().map_or_else(
        |_| chrono::Utc::now().timestamp_nanos(),
        |t| t * 1_000_000_000,
    );

    let duration = duration.parse::<i64>().map_or(-1, |t| t * 1_000_000_000);

    // use nanos, because why the hell not? we won't display them.
    History::new(
        time,
        command.trim_end().to_string(),
        String::from("unknown"),
        -1,
        duration,
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
            Ok(_) => {
                let extended = line.starts_with(':');

                if extended {
                    Some(Ok(parse_extended(line.as_str())))
                } else {
                    Some(Ok(History::new(
                        chrono::Utc::now().timestamp_nanos(), // what else? :/
                        line.trim_end().to_string(),
                        String::from("unknown"),
                        -1,
                        -1,
                        None,
                        None,
                    )))
                }
            }
            Err(e) => Some(Err(e).wrap_err("failed to parse line")),
        }
    }
}

#[cfg(test)]
mod test {
    use super::parse_extended;

    #[test]
    fn test_parse_extended_simple() {
        let parsed = parse_extended(": 1613322469:0;cargo install atuin");

        assert_eq!(parsed.command, "cargo install atuin");
        assert_eq!(parsed.duration, 0);
        assert_eq!(parsed.timestamp, 1_613_322_469_000_000_000);

        let parsed = parse_extended(": 1613322469:10;cargo install atuin;cargo update");

        assert_eq!(parsed.command, "cargo install atuin;cargo update");
        assert_eq!(parsed.duration, 10_000_000_000);
        assert_eq!(parsed.timestamp, 1_613_322_469_000_000_000);

        let parsed = parse_extended(": 1613322469:10;cargo :b̷i̶t̴r̵o̴t̴ ̵i̷s̴ ̷r̶e̵a̸l̷");

        assert_eq!(parsed.command, "cargo :b̷i̶t̴r̵o̴t̴ ̵i̷s̴ ̷r̶e̵a̸l̷");
        assert_eq!(parsed.duration, 10_000_000_000);
        assert_eq!(parsed.timestamp, 1_613_322_469_000_000_000);
    }
}
