// import old shell history!
// automatically hoover up all that we can find

use std::{
    fs::File,
    io::{BufRead, BufReader, Read, Seek},
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
pub struct Zsh<R> {
    file: BufReader<R>,
    strbuf: String,
    loc: usize,
    counter: i64,
}

impl<R: Read + Seek> Zsh<R> {
    fn new(r: R) -> Result<Self> {
        let mut buf = BufReader::new(r);
        let loc = count_lines(&mut buf)?;

        Ok(Self {
            file: buf,
            strbuf: String::new(),
            loc,
            counter: 0,
        })
    }
}

impl Importer for Zsh<File> {
    const NAME: &'static str = "zsh";

    fn histpath() -> Result<PathBuf> {
        // oh-my-zsh sets HISTFILE=~/.zhistory
        // zsh has no default value for this var, but uses ~/.zhistory.
        // we could maybe be smarter about this in the future :)
        let user_dirs = UserDirs::new().unwrap();
        let home_dir = user_dirs.home_dir();

        let mut candidates = [".zhistory", ".zsh_history"].iter();
        loop {
            match candidates.next() {
                Some(candidate) => {
                    let histpath = home_dir.join(candidate);
                    if histpath.exists() {
                        break Ok(histpath);
                    }
                }
                None => break Err(eyre!("Could not find history file. Try setting $HISTFILE")),
            }
        }
    }

    fn parse(path: impl AsRef<Path>) -> Result<Self> {
        Self::new(File::open(path)?)
    }
}

impl<R: Read> Iterator for Zsh<R> {
    type Item = Result<History>;

    fn next(&mut self) -> Option<Self::Item> {
        // ZSH extended history records the timestamp + command duration
        // These lines begin with :
        // So, if the line begins with :, parse it. Otherwise it's just
        // the command
        self.strbuf.clear();
        match self.file.read_line(&mut self.strbuf) {
            Ok(0) => return None,
            Ok(_) => (),
            Err(e) => return Some(Err(eyre!("failed to read line: {}", e))), // we can skip past things like invalid utf8
        }

        self.loc -= 1;

        while self.strbuf.ends_with("\\\\\n") {
            let range = (self.strbuf.len() - 3)..;
            self.strbuf.replace_range(range, "\\\n");

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

        // We have to handle the case where a line has escaped newlines.
        // Keep reading until we have a non-escaped newline

        let extended = self.strbuf.starts_with(':');

        if extended {
            self.counter += 1;
            Some(Ok(parse_extended(&self.strbuf, self.counter)))
        } else {
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
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (0, Some(self.loc))
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

#[cfg(test)]
mod test {
    use std::io::Cursor;

    use chrono::prelude::*;
    use chrono::Utc;

    use super::*;

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

    #[test]
    fn test_parse_file() {
        let input = r": 1613322469:0;cargo install atuin
: 1613322469:10;cargo install atuin; \\
cargo update
: 1613322469:10;cargo :b̷i̶t̴r̵o̴t̴ ̵i̷s̴ ̷r̶e̵a̸l̷
";

        let cursor = Cursor::new(input);
        let mut zsh = Zsh::new(cursor).unwrap();
        assert_eq!(zsh.loc, 4);
        assert_eq!(zsh.size_hint(), (0, Some(4)));

        assert_eq!(&zsh.next().unwrap().unwrap().command, "cargo install atuin");
        assert_eq!(
            &zsh.next().unwrap().unwrap().command,
            "cargo install atuin; \\\ncargo update"
        );
        assert_eq!(
            &zsh.next().unwrap().unwrap().command,
            "cargo :b̷i̶t̴r̵o̴t̴ ̵i̷s̴ ̷r̶e̵a̸l̷"
        );
        assert!(zsh.next().is_none());

        assert_eq!(zsh.size_hint(), (0, Some(0)));
    }
}
