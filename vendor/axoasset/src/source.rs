//! Support for parsing text with richer spanned errors

use std::fmt::Debug;
use std::sync::Arc;

use camino::Utf8Path;
use miette::{MietteSpanContents, SourceCode, SourceSpan};

use crate::{error::*, LocalAsset};

#[cfg(feature = "toml-edit")]
use crate::toml_edit::DocumentMut;

#[cfg(feature = "json-serde")]
use crate::serde_json;

#[cfg(feature = "yaml-serde")]
use crate::serde_yaml_bw;

/// The inner contents of a [`SourceFile`][].
#[derive(Eq, PartialEq)]
struct SourceFileInner {
    /// "Name" of the file
    filename: String,
    /// Origin path of the file
    origin_path: String,
    /// Contents of the file
    contents: String,
}

/// A file's contents along with its display name
///
/// This is used for reporting rustc-style diagnostics where we show
/// where in the file we found a problem. It contains an Arc so that
/// it's ~free for everything to pass/copy these around and produce
/// better diagnostics.
#[derive(Clone, Eq, PartialEq)]
pub struct SourceFile {
    /// The actual impl
    inner: Arc<SourceFileInner>,
}

impl SourceFile {
    /// Create an empty SourceFile with the given name.
    ///
    /// See [`SourceFile::new`][] for details.
    pub fn new_empty(origin_path: &str) -> Self {
        Self::new(origin_path, String::new())
    }

    /// Create a new source file with the given name and contents.
    ///
    /// This is intended for situations where you have the contents already
    /// and just want a SourceFile to manage it. This is appropriate for
    /// strings that were constructed dynamically or for tests.
    ///
    /// The origin_path will be used as the filename as well.
    pub fn new(origin_path: &str, contents: String) -> Self {
        SourceFile {
            inner: Arc::new(SourceFileInner {
                filename: origin_path.to_owned(),
                origin_path: origin_path.to_owned(),
                contents,
            }),
        }
    }

    /// SourceFile equivalent of [`LocalAsset::load_asset`][]
    pub fn load_local(origin_path: impl AsRef<Utf8Path>) -> Result<SourceFile> {
        let origin_path = origin_path.as_ref();
        let contents = LocalAsset::load_string(origin_path)?;
        Ok(SourceFile {
            inner: Arc::new(SourceFileInner {
                filename: crate::local::filename(origin_path)?,
                origin_path: origin_path.to_string(),
                contents,
            }),
        })
    }

    /// Try to deserialize the contents of the SourceFile as json
    #[cfg(feature = "json-serde")]
    pub fn deserialize_json<'a, T: serde::Deserialize<'a>>(&'a self) -> Result<T> {
        // Although many JSON parsers support JSON that begins with a BOM,
        // json-serde doesn't:
        // https://github.com/serde-rs/json/issues/1115
        // In UTF-8, \uFEFF (0xEF 0xBB 0xBF) is always the BOM; it's not
        // variable like in UTF-16. Since the string is already UTF-8 here,
        // stripping the BOM is pretty simple.
        let mut contents = self.contents();
        if let Some(stripped) = contents.strip_prefix('\u{FEFF}') {
            contents = stripped;
        }

        let json = serde_json::from_str(contents).map_err(|details| {
            let span = self.span_for_line_col(details.line(), details.column());
            AxoassetError::Json {
                source: self.clone(),
                span,
                details,
            }
        })?;
        Ok(json)
    }

    /// Try to deserialize the contents of the SourceFile as toml
    #[cfg(feature = "toml-serde")]
    pub fn deserialize_toml<'a, T: for<'de> serde::Deserialize<'de>>(&'a self) -> Result<T> {
        let toml = toml::from_str(self.contents()).map_err(|details| {
            let span = details.span().map(SourceSpan::from);
            AxoassetError::Toml {
                source: self.clone(),
                span,
                details,
            }
        })?;
        Ok(toml)
    }

    /// Try to deserialize the contents of the SourceFile as a toml_edit Document
    #[cfg(feature = "toml-edit")]
    pub fn deserialize_toml_edit(&self) -> Result<DocumentMut> {
        let toml = self.contents().parse::<DocumentMut>().map_err(|details| {
            let span = details.span().map(SourceSpan::from);
            AxoassetError::TomlEdit {
                source: self.clone(),
                span,
                details,
            }
        })?;
        Ok(toml)
    }

    /// Try to deserialize the contents of the SourceFile as yaml
    #[cfg(feature = "yaml-serde")]
    pub fn deserialize_yaml<'a, T: for<'de> serde::Deserialize<'de>>(&self) -> Result<T> {
        let yaml = serde_yaml_bw::from_str(self.contents()).map_err(|details| {
            let span = details
                .location()
                .and_then(|location| self.span_for_line_col(location.line(), location.column()));
            AxoassetError::Yaml {
                source: self.clone(),
                span,
                details,
            }
        })?;
        Ok(yaml)
    }

    /// Get the filename of a SourceFile
    pub fn filename(&self) -> &str {
        &self.inner.filename
    }

    /// Get the origin_path of a SourceFile
    pub fn origin_path(&self) -> &str {
        &self.inner.origin_path
    }

    /// Get the contents of a SourceFile
    pub fn as_str(&self) -> &str {
        &self.inner.contents
    }

    /// Get the contents of a SourceFile (alias for as_str)
    pub fn contents(&self) -> &str {
        &self.inner.contents
    }

    /// Gets a proper [`SourceSpan`] from a line-and-column representation
    ///
    /// Both values are 1's based, so `(1, 1)` is the start of the file.
    /// If anything underflows/overflows or goes out of bounds then we'll
    /// just return `None`. `unwrap_or_default()` will give you the empty span from that.
    ///
    /// This is a pretty heavy-weight process, we have to basically linearly scan the source
    /// for this position!
    pub fn span_for_line_col(&self, line: usize, col: usize) -> Option<SourceSpan> {
        let src = self.contents();
        let src_line = src.lines().nth(line.checked_sub(1)?)?;
        if col > src_line.len() {
            return None;
        }
        let src_addr = src.as_ptr() as usize;
        let line_addr = src_line.as_ptr() as usize;
        let line_offset = line_addr.checked_sub(src_addr)?;
        let start = line_offset.checked_add(col)?.checked_sub(1)?;
        let end = start.checked_add(1)?;
        if start > end || end > src.len() {
            return None;
        }
        Some(SourceSpan::from(start..end))
    }

    /// Creates a span for an item using a substring of `contents`
    ///
    /// Note that substr must be a literal substring, as in it must be
    /// a pointer into the same string! If it's not we'll return None.
    pub fn span_for_substr(&self, substr: &str) -> Option<SourceSpan> {
        // Get the bounds of the full string
        let base_addr = self.inner.contents.as_ptr() as usize;
        let base_len = self.inner.contents.len();

        // Get the bounds of the substring
        let substr_addr = substr.as_ptr() as usize;
        let substr_len = substr.len();

        // The index of the substring is just the number of bytes it is from the start
        // (This will bail out if the """substring""" has an address *before* the full string)
        let start = substr_addr.checked_sub(base_addr)?;
        // The end index (exclusive) is just the start index + sublen
        // (This will bail out if this overflows)
        let end = start.checked_add(substr_len)?;
        // Finally, make sure the substr endpoint isn't past the end of the full string
        if end > base_len {
            return None;
        }

        // At this point it's definitely a substring, nice!
        Some(SourceSpan::from(start..end))
    }
}

impl SourceCode for SourceFile {
    fn read_span<'a>(
        &'a self,
        span: &SourceSpan,
        context_lines_before: usize,
        context_lines_after: usize,
    ) -> std::result::Result<Box<dyn miette::SpanContents<'a> + 'a>, miette::MietteError> {
        let contents =
            self.contents()
                .read_span(span, context_lines_before, context_lines_after)?;
        Ok(Box::new(MietteSpanContents::new_named(
            self.origin_path().to_owned(),
            contents.data(),
            *contents.span(),
            contents.line(),
            contents.column(),
            contents.line_count(),
        )))
    }
}

impl Debug for SourceFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SourceFile")
            .field("origin_path", &self.origin_path())
            .field("contents", &self.contents())
            .finish()
    }
}
