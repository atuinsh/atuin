use std::{
    fs::File,
    io::{BufRead, BufReader, Read, Seek},
    path::{Path, PathBuf},
};

use directories::UserDirs;
use eyre::{eyre, Result};

use super::{count_lines, Importer};
use crate::history::History;

#[derive(Debug)]
pub struct Bash<R> {
    file: BufReader<R>,
    strbuf: String,
    loc: usize,
    counter: i64,
}

impl<R: Read + Seek> Bash<R> {
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

impl Importer for Bash<File> {
    const NAME: &'static str = "bash";

    fn histpath() -> Result<PathBuf> {
        let user_dirs = UserDirs::new().unwrap();
        let home_dir = user_dirs.home_dir();

        Ok(home_dir.join(".bash_history"))
    }

    fn parse(path: &impl AsRef<Path>) -> Result<Self> {
        Self::new(File::open(path)?)
    }
}

impl<R: Read> Iterator for Bash<R> {
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

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::Bash;

    #[test]
    fn test_parse_file() {
        let input = r"cargo install atuin
cargo install atuin; \
cargo update
cargo :b̷i̶t̴r̵o̴t̴ ̵i̷s̴ ̷r̶e̵a̸l̷
";

        let cursor = Cursor::new(input);
        let mut bash = Bash::new(cursor).unwrap();
        assert_eq!(bash.loc, 4);
        assert_eq!(bash.size_hint(), (0, Some(4)));

        assert_eq!(
            &bash.next().unwrap().unwrap().command,
            "cargo install atuin"
        );
        assert_eq!(
            &bash.next().unwrap().unwrap().command,
            "cargo install atuin; \\\ncargo update"
        );
        assert_eq!(
            &bash.next().unwrap().unwrap().command,
            "cargo :b̷i̶t̴r̵o̴t̴ ̵i̷s̴ ̷r̶e̵a̸l̷"
        );
        assert!(bash.next().is_none());

        assert_eq!(bash.size_hint(), (0, Some(0)));
    }
}
