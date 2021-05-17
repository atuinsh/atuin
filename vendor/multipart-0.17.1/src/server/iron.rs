//! Integration with the [Iron](https://ironframework.io) framework, enabled with the `iron` feature (optional). Includes a `BeforeMiddleware` implementation.
//!
//! Not shown here: `impl `[`HttpRequest`](../trait.HttpRequest.html#implementors)` for
//! iron::Request`.

use iron::headers::ContentType;
use iron::mime::{Mime, TopLevel, SubLevel};
use iron::request::{Body as IronBody, Request as IronRequest};
use iron::typemap::Key;
use iron::{BeforeMiddleware, IronError, IronResult};

use std::path::PathBuf;
use std::{error, fmt, io};
use tempfile;

use super::{FieldHeaders, HttpRequest, Multipart};
use super::save::{Entries, PartialReason, TempDir};
use super::save::SaveResult::*;

impl<'r, 'a, 'b> HttpRequest for &'r mut IronRequest<'a, 'b> {
    type Body = &'r mut IronBody<'a, 'b>;

    fn multipart_boundary(&self) -> Option<&str> {
        let content_type = try_opt!(self.headers.get::<ContentType>());
        if let Mime(TopLevel::Multipart, SubLevel::FormData, _) = **content_type {
            content_type.get_param("boundary").map(|b| b.as_str())
        } else {
            None
        }
    }

    fn body(self) -> &'r mut IronBody<'a, 'b> {
        &mut self.body
    }
}

/// The default file size limit for [`Intercept`](struct.Intercept.html), in bytes.
pub const DEFAULT_FILE_SIZE_LIMIT: u64 = 2 * 1024 * 1024;

/// The default file count limit for [`Intercept`](struct.Intercept.html).
pub const DEFAULT_FILE_COUNT_LIMIT: u32 = 16;

/// A `BeforeMiddleware` for Iron which will intercept and read-out multipart requests and store
/// the result in the request.
///
/// Successful reads will be placed in the `extensions: TypeMap` field of `iron::Request` as an
/// [`Entries`](../struct.Entries.html) instance (as both key-type and value):
///
/// ```no_run
/// extern crate iron;
/// extern crate multipart;
///
/// use iron::prelude::*;
///
/// use multipart::server::Entries;
/// use multipart::server::iron::Intercept;
///
/// fn main() {
///     let mut chain = Chain::new(|req: &mut Request| if let Some(entries) =
///         req.extensions.get::<Entries>() {
///
///         Ok(Response::with(format!("{:?}", entries)))
///     } else {
///         Ok(Response::with("Not a multipart request"))
///     });
///
///     chain.link_before(Intercept::default());
///
///     Iron::new(chain).http("localhost:80").unwrap();
/// }
/// ```
///
/// Any errors during which occur during reading will be passed on as `IronError`.
#[derive(Debug)]
pub struct Intercept {
    /// The parent directory for all temporary directories created by this middleware.
    /// Will be created if it doesn't exist (lazy).
    ///
    /// If omitted, uses the OS temporary directory.
    ///
    /// Default value: `None`.
    pub temp_dir_path: Option<PathBuf>,
    /// The size limit of uploaded files, in bytes.
    ///
    /// Files which exceed this size will be rejected.
    /// See the `limit_behavior` field for more info.
    ///
    /// Default value: [`DEFAULT_FILE_SIZE_LIMIT`](constant.default_file_size_limit.html)
    pub file_size_limit: u64,
    /// The limit on the number of files which will be saved from
    /// the request. Requests which exceed this count will be rejected.
    ///
    /// Default value: [`DEFAULT_FILE_COUNT_LIMT`](constant.default_file_count_limit.html)
    pub file_count_limit: u32,
    /// What to do when a file count or size limit has been exceeded.
    ///
    /// See [`LimitBehavior`](enum.limitbehavior.html) for more info.
    pub limit_behavior: LimitBehavior,
}

impl Intercept {
    /// Set the `temp_dir_path` for this middleware.
    pub fn temp_dir_path<P: Into<PathBuf>>(self, path: P) -> Self {
        Intercept { temp_dir_path: Some(path.into()), .. self }
    }

    /// Set the `file_size_limit` for this middleware.
    pub fn file_size_limit(self, limit: u64) -> Self {
        Intercept { file_size_limit: limit, .. self }
    }

    /// Set the `file_count_limit` for this middleware.
    pub fn file_count_limit(self, limit: u32) -> Self {
        Intercept { file_count_limit: limit, .. self }
    }

    /// Set the `limit_behavior` for this middleware.
    pub fn limit_behavior(self, behavior: LimitBehavior) -> Self {
        Intercept { limit_behavior: behavior, .. self }
    }

    fn read_request(&self, req: &mut IronRequest) -> IronResult<Option<Entries>> {
        let multipart = match Multipart::from_request(req) {
            Ok(multipart) => multipart,
            Err(_) => return Ok(None),
        };

        let tempdir = self.temp_dir_path.as_ref()
                .map_or_else(
                    || tempfile::Builder::new().prefix("multipart-iron").tempdir(),
                    |path| tempfile::Builder::new().prefix("multipart-iron").tempdir_in(path)
                )
                .map_err(|e| io_to_iron(e, "Error opening temporary directory for request."))?;

        match self.limit_behavior {
            LimitBehavior::ThrowError => self.read_request_strict(multipart, tempdir),
            LimitBehavior::Continue => self.read_request_lenient(multipart, tempdir),
        }
    }

    fn read_request_strict(&self, mut multipart: IronMultipart, tempdir: TempDir) -> IronResult<Option<Entries>> {
        match multipart.save().size_limit(self.file_size_limit)
                              .count_limit(self.file_count_limit)
                              .with_temp_dir(tempdir) {
            Full(entries) => Ok(Some(entries)),
            Partial(_, PartialReason::Utf8Error(_)) => unreachable!(),
            Partial(_, PartialReason::IoError(err)) => Err(io_to_iron(err, "Error midway through request")),
            Partial(_, PartialReason::CountLimit) => Err(FileCountLimitError(self.file_count_limit).into()),
            Partial(partial, PartialReason::SizeLimit) =>  {
                let partial = partial.partial.expect(EXPECT_PARTIAL_FILE);
                Err(
                    FileSizeLimitError {
                        field: partial.source.headers,
                    }.into()
                )
            },
            Error(err) => Err(io_to_iron(err, "Error at start of request")),
        }
    }

    fn read_request_lenient(&self, mut multipart: IronMultipart, tempdir: TempDir) -> IronResult<Option<Entries>> {
        let mut entries = match multipart.save().size_limit(self.file_size_limit)
                                                .count_limit(self.file_count_limit)
                                                .with_temp_dir(tempdir) {
            Full(entries) => return Ok(Some(entries)),
            Partial(_, PartialReason::IoError(err)) => return Err(io_to_iron(err, "Error midway through request")),
            Partial(partial, _) =>  partial.keep_partial(),
            Error(err) => return Err(io_to_iron(err, "Error at start of request")),
        };

        loop {
            entries = match multipart.save().size_limit(self.file_size_limit)
                                            .count_limit(self.file_count_limit)
                                            .with_entries(entries) {
                Full(entries) => return Ok(Some(entries)),
                Partial(_, PartialReason::IoError(err)) => return Err(io_to_iron(err, "Error midway through request")),
                Partial(partial, _) => partial.keep_partial(),
                Error(err) => return Err(io_to_iron(err, "Error at start of request")),
            };
        }
    }
}

type IronMultipart<'r, 'a, 'b> = Multipart<&'r mut IronBody<'a, 'b>>;

const EXPECT_PARTIAL_FILE: &str = "File size limit hit but the offending \
                                   file was not available; this is a bug.";

impl Default for Intercept {
    fn default() -> Self {
        Intercept {
            temp_dir_path: None,
            file_size_limit: DEFAULT_FILE_SIZE_LIMIT,
            file_count_limit: DEFAULT_FILE_COUNT_LIMIT,
            limit_behavior: LimitBehavior::ThrowError,
        }
    }
}

impl BeforeMiddleware for Intercept {
    fn before(&self, req: &mut IronRequest) -> IronResult<()> {
        self.read_request(req)?
            .map(|entries| req.extensions.insert::<Entries>(entries));

        Ok(())
    }
}

impl Key for Entries {
    type Value = Self;
}

/// The behavior of `Intercept` when a file size or count limit is exceeded.
#[derive(Clone, Copy, Debug)]
#[repr(u32)]
pub enum LimitBehavior {
    /// Return an error from the middleware describing the issue.
    ThrowError,
    /// Ignore the limit.
    ///
    /// In the case of file size limits, the offending file will be truncated
    /// in the result.
    ///
    /// In the case of file count limits, the request will be completed.
    Continue,
}

/// An error returned from `Intercept` when the size limit
/// for an individual file is exceeded.
#[derive(Debug)]
pub struct FileSizeLimitError {
    /// The field where the error occurred.
    pub field: FieldHeaders,
}

impl error::Error for FileSizeLimitError {
    fn description(&self) -> &str {
        "file size limit reached"
    }
}

impl fmt::Display for FileSizeLimitError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self.field.filename {
            Some(ref filename) => write!(f, "File size limit reached for field \"{}\" (filename: \"{}\")", self.field.name, filename),
            None => write!(f, "File size limit reached for field \"{}\" (no filename)", self.field.name),
        }
    }
}

impl Into<IronError> for FileSizeLimitError {
    fn into(self) -> IronError {
        let desc_str = self.to_string();
        IronError::new(self, desc_str)
    }
}

/// An error returned from `Intercept` when the file count limit
/// for a single request was exceeded.
#[derive(Debug)]
pub struct FileCountLimitError(u32);

impl error::Error for FileCountLimitError {
    fn description(&self) -> &str {
        "file count limit reached"
    }
}

impl fmt::Display for FileCountLimitError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "File count limit reached for request. Limit: {}", self.0)
    }
}

impl Into<IronError> for FileCountLimitError {
    fn into(self) -> IronError {
        let desc_string = self.to_string();
        IronError::new(self, desc_string)
    }
}

fn io_to_iron<M: Into<String>>(err: io::Error, msg: M) -> IronError {
    IronError::new(err, msg.into())
}
