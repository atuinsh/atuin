use std::{fmt,io};
use std::error::Error;

/// a scanner error type
#[derive(Debug)]
#[derive(PartialEq)]
pub struct ScanError {
    pub details: String,
    pub lineno: u32,
}

impl fmt::Display for ScanError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f,"line {}: {}",self.lineno,self.details)
    }
}

impl ScanError {
    /// create a new error
    pub fn new(msg: &str) -> ScanError {
        ScanError{details: msg.into(), lineno: 1}
    }
}

impl Error for ScanError {}

impl From<io::Error> for ScanError {
    fn from(err: io::Error) -> ScanError {
        ScanError::new(&err.to_string())
    }
}
