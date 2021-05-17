use std::error::Error;
use std::result;
use std::str::FromStr;

use super::{FileFormat, FileSource};
use source::Source;

/// Describes a file sourced from a string
#[derive(Clone, Debug)]
pub struct FileSourceString(String);

impl<'a> From<&'a str> for FileSourceString {
    fn from(s: &'a str) -> Self {
        FileSourceString(s.into())
    }
}

impl FileSource for FileSourceString {
    fn resolve(
        &self,
        format_hint: Option<FileFormat>,
    ) -> Result<(Option<String>, String, FileFormat), Box<dyn Error + Send + Sync>> {
        Ok((
            None,
            self.0.clone(),
            format_hint.expect("from_str requires a set file format"),
        ))
    }
}
