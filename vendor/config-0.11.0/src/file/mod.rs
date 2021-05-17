mod format;
pub mod source;

use error::*;
use source::Source;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use value::Value;

pub use self::format::FileFormat;
use self::source::FileSource;

pub use self::source::file::FileSourceFile;
pub use self::source::string::FileSourceString;

#[derive(Clone, Debug)]
pub struct File<T>
where
    T: FileSource,
{
    source: T,

    /// Format of file (which dictates what driver to use).
    format: Option<FileFormat>,

    /// A required File will error if it cannot be found
    required: bool,
}

impl File<source::string::FileSourceString> {
    pub fn from_str(s: &str, format: FileFormat) -> Self {
        File {
            format: Some(format),
            required: true,
            source: s.into(),
        }
    }
}

impl File<source::file::FileSourceFile> {
    pub fn new(name: &str, format: FileFormat) -> Self {
        File {
            format: Some(format),
            required: true,
            source: source::file::FileSourceFile::new(name.into()),
        }
    }

    /// Given the basename of a file, will attempt to locate a file by setting its
    /// extension to a registered format.
    pub fn with_name(name: &str) -> Self {
        File {
            format: None,
            required: true,
            source: source::file::FileSourceFile::new(name.into()),
        }
    }
}

impl<'a> From<&'a Path> for File<source::file::FileSourceFile> {
    fn from(path: &'a Path) -> Self {
        File {
            format: None,
            required: true,
            source: source::file::FileSourceFile::new(path.to_path_buf()),
        }
    }
}

impl From<PathBuf> for File<source::file::FileSourceFile> {
    fn from(path: PathBuf) -> Self {
        File {
            format: None,
            required: true,
            source: source::file::FileSourceFile::new(path),
        }
    }
}

impl<T: FileSource> File<T> {
    pub fn format(mut self, format: FileFormat) -> Self {
        self.format = Some(format);
        self
    }

    pub fn required(mut self, required: bool) -> Self {
        self.required = required;
        self
    }
}

impl<T: FileSource> Source for File<T>
where
    T: 'static,
    T: Sync + Send,
{
    fn clone_into_box(&self) -> Box<dyn Source + Send + Sync> {
        Box::new((*self).clone())
    }

    fn collect(&self) -> Result<HashMap<String, Value>> {
        // Coerce the file contents to a string
        let (uri, contents, format) = match self
            .source
            .resolve(self.format)
            .map_err(|err| ConfigError::Foreign(err))
        {
            Ok((uri, contents, format)) => (uri, contents, format),

            Err(error) => {
                if !self.required {
                    return Ok(HashMap::new());
                }

                return Err(error);
            }
        };

        // Parse the string using the given format
        format
            .parse(uri.as_ref(), &contents)
            .map_err(|cause| ConfigError::FileParse { uri, cause })
    }
}
