use std::collections::HashMap;
use std::env;
use std::io::{BufReader, Lines};
use std::io::prelude::*;

use crate::errors::*;
use crate::parse;

pub struct Iter<R> {
    lines: Lines<BufReader<R>>,
    substitution_data: HashMap<String, Option<String>>,
}

impl<R: Read> Iter<R> {
    pub fn new(reader: R) -> Iter<R> {
        Iter {
            lines: BufReader::new(reader).lines(),
            substitution_data: HashMap::new(),
        }
    }

    pub fn load(self) -> Result<()> {
        for item in self {
            let (key, value) = item?;
            if env::var(&key).is_err() {
                env::set_var(&key, value);
            }
        }

        Ok(())
    }
}

impl<R: Read> Iterator for Iter<R> {
    type Item = Result<(String, String)>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let line = match self.lines.next() {
                Some(Ok(line)) => line,
                Some(Err(err)) => return Some(Err(Error::Io(err))),
                None => return None,
            };

            match parse::parse_line(&line, &mut self.substitution_data) {
                Ok(Some(result)) => return Some(Ok(result)),
                Ok(None) => {}
                Err(err) => return Some(Err(err)),
            }
        }
    }
}
