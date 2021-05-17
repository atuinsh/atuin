pub mod file;
pub mod string;

use std::error::Error;
use std::fmt::Debug;

use super::FileFormat;
use source::Source;

/// Describes where the file is sourced
pub trait FileSource: Debug + Clone {
    fn resolve(
        &self,
        format_hint: Option<FileFormat>,
    ) -> Result<(Option<String>, String, FileFormat), Box<dyn Error + Send + Sync>>;
}
