use scanlex::ScanError;
use std::error::Error;
use std::fmt;

#[derive(Debug)]
pub struct DateError {
    details: String,
}

impl fmt::Display for DateError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f,"{}",self.details)
    }
}

impl Error for DateError {}

pub type DateResult<T> = Result<T,DateError>;

pub fn date_error(msg: &str) -> DateError {
    DateError{details: msg.into()}
}

pub fn date_result<T>(msg: &str) -> DateResult<T> {
    Err(date_error(msg).into())
}

impl From<ScanError> for DateError {
    fn from(err: ScanError) -> DateError {
        date_error(&err.to_string())
    }
}


/// This trait maps optional values onto `DateResult`
pub trait OrErr<T> {
    /// use when the error message is always a simple string
    fn or_err(self, msg: &str) -> DateResult<T>;

    /// use when the message needs to be constructed
    fn or_then_err<C: FnOnce()->String>(self,fun:C) -> DateResult<T>;
}

impl <T>OrErr<T> for Option<T> {
    fn or_err(self, msg: &str) -> DateResult<T> {
        self.ok_or(date_error(msg))
    }

    fn or_then_err<C: FnOnce()->String>(self,fun:C) -> DateResult<T> {
        self.ok_or_else(|| date_error(&fun()))
    }
}



