// Copyright 2016 `multipart` Crate Developers
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.
//! Utilities for saving request entries to the filesystem.

pub use server::buf_redux::BufReader;

pub use tempfile::TempDir;

use std::collections::HashMap;
use std::io::prelude::*;
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::{cmp, env, io, mem, str, u32, u64};
use tempfile;

use server::field::{FieldHeaders, MultipartField, MultipartData, ReadEntry, ReadEntryResult};

use self::SaveResult::*;
use self::TextPolicy::*;
use self::PartialReason::*;

const RANDOM_FILENAME_LEN: usize = 12;

fn rand_filename() -> String {
    ::random_alphanumeric(RANDOM_FILENAME_LEN)
}

macro_rules! try_start (
    ($try:expr) => (
        match $try {
            Ok(val) => val,
            Err(e) => return Error(e),
        }
    )
);

macro_rules! try_full (
    ($try:expr) => {
        match $try {
            Full(full) => full,
            other => return other,
        }
    }
);

macro_rules! try_partial (
    ($try:expr) => {
        match $try {
            Full(full) => return Full(full.into()),
            Partial(partial, reason) => (partial, reason),
            Error(e) => return Error(e),
        }
    }
);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TextPolicy {
    /// Attempt to read a text field as text, falling back to binary on error
    Try,
    /// Attempt to read a text field as text, returning any errors
    Force,
    /// Don't try to read text
    Ignore
}

/// A builder for saving a file or files to the local filesystem.
///
/// ### `OpenOptions`
/// This builder holds an instance of `std::fs::OpenOptions` which is used
/// when creating the new file(s).
///
/// By default, the open options are set with `.write(true).create_new(true)`,
/// so if the file already exists then an error will be thrown. This is to avoid accidentally
/// overwriting files from other requests.
///
/// If you want to modify the options used to open the save file, you can use
/// `mod_open_opts()`.
///
/// ### File Size and Count Limits
/// You can set a size limit for individual fields with `size_limit()`, which takes either `u64`
/// or `Option<u64>`.
///
/// You can also set the maximum number of fields to process with `count_limit()`, which
/// takes either `u32` or `Option<u32>`. This only has an effect when using
/// `SaveBuilder<[&mut] Multipart>`.
///
/// By default, these limits are set conservatively to limit the maximum memory and disk space
/// usage of a single request. You should set `count_limit` specifically for each request endpoint
/// based on the number of fields you're expecting (exactly to that number if you're not expecting
/// duplicate fields).
///
/// ### Memory Threshold and Text Policy
/// By default, small fields (a few kilobytes or smaller) will be read directly to memory
/// without creating a file. This behavior is controlled by the `memory_threshold()` setter. You can
/// *roughly* tune the maximum memory a single request uses by tuning
/// `count_limit * memory_threshold`
///
/// If a field appears to contain text data (its content-type is `text/*` or it doesn't declare
/// one), `SaveBuilder` can read it to a string instead of saving the raw bytes as long as it falls
/// below the set `memory_threshold`.
///
/// By default, the behavior is to attempt to validate the data as UTF-8, falling back to saving
/// just the bytes if the validation fails at any point. You can restore/ensure this behavior
/// with the `try_text()` modifier.
///
/// Alternatively, you can use the `force_text()` modifier to make the save operation return
/// an error when UTF-8 decoding fails, though this only holds true while the size is below
/// `memory_threshold`. The `ignore_text()` modifier turns off UTF-8 validation altogether.
///
/// UTF-8 validation is performed incrementally (after every `BufRead::fill_buf()` call)
/// to hopefully maximize throughput, instead of blocking while the field is read to completion
/// and performing validation over the entire result at the end. (RFC: this could be a lot of
/// unnecessary work if most fields end up being written to the filesystem, however, but this
/// can be turned off with `ignore_text()` if it fits the use-case.)
///
/// ### Warning: Do **not** trust user input!
/// It is a serious security risk to create files or directories with paths based on user input.
/// A malicious user could craft a path which can be used to overwrite important files, such as
/// web templates, static assets, Javascript files, database files, configuration files, etc.,
/// if they are writable by the server process.
///
/// This can be mitigated somewhat by setting filesystem permissions as
/// conservatively as possible and running the server under its own user with restricted
/// permissions, but you should still not use user input directly as filesystem paths.
/// If it is truly necessary, you should sanitize user input such that it cannot cause a path to be
/// misinterpreted by the OS. Such functionality is outside the scope of this crate.
#[must_use = "nothing saved to the filesystem yet"]
pub struct SaveBuilder<S> {
    savable: S,
    open_opts: OpenOptions,
    size_limit: u64,
    count_limit: u32,
    memory_threshold: u64,
    text_policy: TextPolicy,
}

/// Common methods for whole requests as well as individual fields.
impl<S> SaveBuilder<S> {
    /// Implementation detail but not problematic to have accessible.
    #[doc(hidden)]
    pub fn new(savable: S) -> SaveBuilder<S> {
        let mut open_opts = OpenOptions::new();
        open_opts.write(true).create_new(true);

        SaveBuilder {
            savable,
            open_opts,
            // 8 MiB, on the conservative end compared to most frameworks
            size_limit: 8 * 1024 * 1024,
            // Arbitrary, I have no empirical data for this
            count_limit: 256,
            // 10KiB, used by Apache Commons
            // https://commons.apache.org/proper/commons-fileupload/apidocs/org/apache/commons/fileupload/disk/DiskFileItemFactory.html
            memory_threshold: 10 * 1024,
            text_policy: TextPolicy::Try,
        }
    }

    /// Set the maximum number of bytes to write out *per file*.
    ///
    /// Can be `u64` or `Option<u64>`. If `None` or `u64::MAX`, clears the limit.
    pub fn size_limit<L: Into<Option<u64>>>(mut self, limit: L) -> Self {
        self.size_limit = limit.into().unwrap_or(u64::MAX);
        self
    }

    /// Modify the `OpenOptions` used to open any files for writing.
    ///
    /// The `write` flag will be reset to `true` after the closure returns. (It'd be pretty
    /// pointless otherwise, right?)
    pub fn mod_open_opts<F: FnOnce(&mut OpenOptions)>(mut self, opts_fn: F) -> Self {
        opts_fn(&mut self.open_opts);
        self.open_opts.write(true);
        self
    }

    /// Set the threshold at which to switch from copying a field into memory to copying
    /// it to disk.
    ///
    /// If `0`, forces fields to save directly to the filesystem.
    /// If `u64::MAX`, effectively forces fields to always save to memory.
    pub fn memory_threshold(self, memory_threshold: u64) -> Self {
        Self { memory_threshold, ..self }
    }

    /// When encountering a field that is apparently text, try to read it to a string or fall
    /// back to binary otherwise.
    ///
    /// If set for an individual field (`SaveBuilder<&mut MultipartData<_>>`), will
    /// always attempt to decode text regardless of the field's `Content-Type`.
    ///
    /// Has no effect once `memory_threshold` has been reached.
    pub fn try_text(self) -> Self {
        Self { text_policy: TextPolicy::Try, ..self }
    }

    /// When encountering a field that is apparently text, read it to a string or return an error.
    ///
    /// If set for an individual field (`SaveBuilder<&mut MultipartData<_>>`), will
    /// always attempt to decode text regardless of the field's `Content-Type`.
    ///
    /// (RFC: should this continue to validate UTF-8 when writing to the filesystem?)
    pub fn force_text(self) -> Self {
        Self { text_policy: TextPolicy::Force, ..self}
    }

    /// Don't try to read or validate any field data as UTF-8.
    pub fn ignore_text(self) -> Self {
        Self { text_policy: TextPolicy::Ignore, ..self }
    }
}

/// Save API for whole multipart requests.
impl<M> SaveBuilder<M> where M: ReadEntry {
    /// Set the maximum number of fields to process.
    ///
    /// Can be `u32` or `Option<u32>`. If `None` or `u32::MAX`, clears the limit.
    pub fn count_limit<L: Into<Option<u32>>>(mut self, count_limit: L) -> Self {
        self.count_limit = count_limit.into().unwrap_or(u32::MAX);
        self
    }

    /// Save all fields in the request using a new temporary directory prefixed with
    /// `multipart-rs` in the OS temporary directory.
    ///
    /// For more options, create a `TempDir` yourself and pass it to `with_temp_dir()` instead.
    ///
    /// See `with_entries()` for more info.
    ///
    /// ### Note: Temporary
    /// See `SaveDir` for more info (the type of `Entries::save_dir`).
    pub fn temp(self) -> EntriesSaveResult<M> {
        self.temp_with_prefix("multipart-rs")
    }

    /// Save all fields in the request using a new temporary directory with the given string
    /// as a prefix in the OS temporary directory.
    ///
    /// For more options, create a `TempDir` yourself and pass it to `with_temp_dir()` instead.
    ///
    /// See `with_entries()` for more info.
    ///
    /// ### Note: Temporary
    /// See `SaveDir` for more info (the type of `Entries::save_dir`).
    pub fn temp_with_prefix(self, prefix: &str) -> EntriesSaveResult<M> {
        match tempfile::Builder::new().prefix(prefix).tempdir() {
            Ok(tempdir) => self.with_temp_dir(tempdir),
            Err(e) => SaveResult::Error(e),
        }
    }

    /// Save all fields in the request using the given `TempDir`.
    ///
    /// See `with_entries()` for more info.
    ///
    /// The `TempDir` is returned in the result under `Entries::save_dir`.
    pub fn with_temp_dir(self, tempdir: TempDir) -> EntriesSaveResult<M> {
        self.with_entries(Entries::new(SaveDir::Temp(tempdir)))
    }

    /// Save the file fields in the request to a new permanent directory with the given path.
    ///
    /// Any nonexistent directories in the path will be created.
    ///
    /// See `with_entries()` for more info.
    pub fn with_dir<P: Into<PathBuf>>(self, dir: P) -> EntriesSaveResult<M> {
        let dir = dir.into();

        try_start!(create_dir_all(&dir));

        self.with_entries(Entries::new(SaveDir::Perm(dir)))
    }

    /// Commence the save operation using the existing `Entries` instance.
    ///
    /// May be used to resume a saving operation after handling an error.
    ///
    /// If `count_limit` is set, only reads that many fields before returning an error.
    /// If you wish to resume from `PartialReason::CountLimit`, simply remove some entries.
    ///
    /// Note that `PartialReason::CountLimit` will still be returned if the number of fields
    /// reaches `u32::MAX`, but this would be an extremely degenerate case.
    pub fn with_entries(self, mut entries: Entries) -> EntriesSaveResult<M> {
        let SaveBuilder {
            savable, open_opts, count_limit, size_limit,
            memory_threshold, text_policy
        } = self;

        let mut res = ReadEntry::read_entry(savable);

        let _ = entries.recount_fields();

        let save_field = |field: &mut MultipartField<M>, entries: &Entries| {
            let text_policy = if field.is_text() { text_policy } else { Ignore };

            let mut saver = SaveBuilder {
                savable: &mut field.data, open_opts: open_opts.clone(),
                count_limit, size_limit, memory_threshold, text_policy
            };

            saver.with_dir(entries.save_dir.as_path())
        };

        while entries.fields_count < count_limit {
            let mut field: MultipartField<M> = match res {
                ReadEntryResult::Entry(field) => field,
                ReadEntryResult::End(_) => return Full(entries), // normal exit point
                ReadEntryResult::Error(_, e) => return Partial (
                    PartialEntries {
                        entries,
                        partial: None,
                    },
                    e.into(),
                )
            };

            let (dest, reason) = match save_field(&mut field, &entries) {
                Full(saved) => {
                    entries.push_field(field.headers, saved);
                    res = ReadEntry::read_entry(field.data.into_inner());
                    continue;
                },
                Partial(saved, reason) => (Some(saved), reason),
                Error(error) => (None, PartialReason::IoError(error)),
            };

            return Partial(
                PartialEntries {
                    entries,
                    partial: Some(PartialSavedField {
                        source: field,
                        dest,
                    }),
                },
                reason
            );
        }

        Partial(
            PartialEntries {
                entries,
                partial: None,
            },
            PartialReason::CountLimit
        )
    }
}

/// Save API for individual fields.
impl<'m, M: 'm> SaveBuilder<&'m mut MultipartData<M>> where MultipartData<M>: BufRead {
    /// Save the field data, potentially using a file with a random name in the
    /// OS temporary directory.
    ///
    /// See `with_path()` for more details.
    pub fn temp(&mut self) -> FieldSaveResult {
        let path = env::temp_dir().join(rand_filename());
        self.with_path(path)
    }

    /// Save the field data, potentially using a file with the given name in
    /// the OS temporary directory.
    ///
    /// See `with_path()` for more details.
    pub fn with_filename(&mut self, filename: &str) -> FieldSaveResult {
        let mut tempdir = env::temp_dir();
        tempdir.set_file_name(filename);

        self.with_path(tempdir)
    }

    /// Save the field data, potentially using a file with a random alphanumeric name
    /// in the given directory.
    ///
    /// See `with_path()` for more details.
    pub fn with_dir<P: AsRef<Path>>(&mut self, dir: P) -> FieldSaveResult {
        let path = dir.as_ref().join(rand_filename());
        self.with_path(path)
    }

    /// Save the field data, potentially using a file with the given path.
    ///
    /// Creates any missing directories in the path (RFC: skip this step?).
    /// Uses the contained `OpenOptions` to create the file.
    /// Truncates the file to the given `size_limit`, if set.
    ///
    /// The no directories or files will be created until the set `memory_threshold` is reached.
    /// If `size_limit` is set and less than or equal to `memory_threshold`,
    /// then the disk will never be touched.
    pub fn with_path<P: Into<PathBuf>>(&mut self, path: P) -> FieldSaveResult {
        let bytes = if self.text_policy != Ignore {
            let (text, reason) = try_partial!(self.save_text());
            match reason {
                SizeLimit if !self.cmp_size_limit(text.len()) => text.into_bytes(),
                Utf8Error(_) if self.text_policy != Force => text.into_bytes(),
                other => return Partial(text.into(), other),
            }
        } else {
            Vec::new()
        };

        let (bytes, reason) = try_partial!(self.save_mem(bytes));

        match reason {
            SizeLimit if !self.cmp_size_limit(bytes.len()) => (),
            other => return Partial(bytes.into(), other)
        }

        let path = path.into();

        let mut file = match create_dir_all(&path).and_then(|_| self.open_opts.open(&path)) {
            Ok(file) => file,
            Err(e) => return Error(e),
        };

        let data = try_full!(
            try_write_all(&bytes, &mut file)
                .map(move |size| SavedData::File(path, size as u64))
        );

        self.write_to(file).map(move |written| data.add_size(written))
    }


    /// Write out the field data to `dest`, truncating if a limit was set.
    ///
    /// Returns the number of bytes copied, and whether or not the limit was reached
    /// (tested by `MultipartFile::fill_buf().is_empty()` so no bytes are consumed).
    ///
    /// Retries on interrupts.
    pub fn write_to<W: Write>(&mut self, mut dest: W) -> SaveResult<u64, u64> {
        if self.size_limit < u64::MAX {
            try_copy_limited(&mut self.savable, |buf| try_write_all(buf, &mut dest), self.size_limit)
        } else {
            try_read_buf(&mut self.savable, |buf| try_write_all(buf, &mut dest))
        }
    }

    fn save_mem(&mut self, mut bytes: Vec<u8>) -> SaveResult<Vec<u8>, Vec<u8>> {
        let pre_read = bytes.len() as u64;
        match self.read_mem(|buf| { bytes.extend_from_slice(buf); Full(buf.len()) }, pre_read) {
            Full(_) => Full(bytes),
            Partial(_, reason) => Partial(bytes, reason),
            Error(e) => if !bytes.is_empty() { Partial(bytes, e.into()) }
            else { Error(e) }
        }

    }

    fn save_text(&mut self) -> SaveResult<String, String> {
        let mut string = String::new();

        // incrementally validate UTF-8 to do as much work as possible during network activity
        let res = self.read_mem(|buf| {
            match str::from_utf8(buf) {
                Ok(s) => { string.push_str(s); Full(buf.len()) },
                // buffer should always be bigger
                Err(e) => if buf.len() < 4 {
                        Partial(0, e.into())
                    } else {
                        string.push_str(str::from_utf8(&buf[..e.valid_up_to()]).unwrap());
                        Full(e.valid_up_to())
                    }
            }
        }, 0);

        match res {
            Full(_) => Full(string),
            Partial(_, reason) => Partial(string, reason),
            Error(e) => Error(e),
        }
    }

    fn read_mem<Wb: FnMut(&[u8]) -> SaveResult<usize, usize>>(&mut self, with_buf: Wb, pre_read: u64) -> SaveResult<u64, u64> {
        let limit = cmp::min(self.size_limit, self.memory_threshold)
            .saturating_sub(pre_read);
        try_copy_limited(&mut self.savable, with_buf, limit)
    }

    fn cmp_size_limit(&self, size: usize) -> bool {
        size as u64 >= self.size_limit
    }
}

/// A field that has been saved (to memory or disk) from a multipart request.
#[derive(Debug)]
pub struct SavedField {
    /// The headers of the field that was saved.
    pub headers: FieldHeaders,
    /// The data of the field which may reside in memory or on disk.
    pub data: SavedData,
}

/// A saved field's data container (in memory or on disk)
#[derive(Debug)]
pub enum SavedData {
    /// Validated UTF-8 text data.
    Text(String),
    /// Binary data.
    Bytes(Vec<u8>),
    /// A path to a file on the filesystem and its size as written by `multipart`.
    File(PathBuf, u64),
}

impl SavedData {
    /// Get an adapter for this data which implements `Read`.
    ///
    /// If the data is in a file, the file is opened in read-only mode.
    pub fn readable(&self) -> io::Result<DataReader> {
        use self::SavedData::*;

        match *self {
            Text(ref text) => Ok(DataReader::Bytes(text.as_ref())),
            Bytes(ref bytes) => Ok(DataReader::Bytes(bytes)),
            File(ref path, _) => Ok(DataReader::File(BufReader::new(fs::File::open(path)?))),
        }
    }

    /// Get the size of the data, in memory or on disk.
    ///
    /// #### Note
    /// The size on disk may not match the size of the file if it is externally modified.
    pub fn size(&self) -> u64 {
        use self::SavedData::*;

        match *self {
            Text(ref text) => text.len() as u64,
            Bytes(ref bytes) => bytes.len() as u64,
            File(_, size) => size,
        }
    }

    /// Returns `true` if the data is known to be in memory (`Text | Bytes`)
    pub fn is_memory(&self) -> bool {
        use self::SavedData::*;

        match *self {
            Text(_) | Bytes(_) => true,
            File(_, _) => false,
        }
    }

    fn add_size(self, add: u64) -> Self {
        use self::SavedData::File;

        match self {
            File(path, size) => File(path, size.saturating_add(add)),
            other => other
        }
    }
}

impl From<String> for SavedData {
    fn from(s: String) -> Self {
        SavedData::Text(s)
    }
}

impl From<Vec<u8>> for SavedData {
    fn from(b: Vec<u8>) -> Self {
        SavedData::Bytes(b)
    }
}

/// A `Read` (and `BufRead`) adapter for `SavedData`
pub enum DataReader<'a> {
    /// In-memory data source (`SavedData::Bytes | Text`)
    Bytes(&'a [u8]),
    /// On-disk data source (`SavedData::File`)
    File(BufReader<File>),
}

impl<'a> Read for DataReader<'a> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        use self::DataReader::*;

        match *self {
            Bytes(ref mut bytes) => bytes.read(buf),
            File(ref mut file) => file.read(buf),
        }
    }
}

impl<'a> BufRead for DataReader<'a> {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        use self::DataReader::*;

        match *self {
            Bytes(ref mut bytes) => bytes.fill_buf(),
            File(ref mut file) => file.fill_buf(),
        }
    }

    fn consume(&mut self, amt: usize) {
        use self::DataReader::*;

        match *self {
            Bytes(ref mut bytes) => bytes.consume(amt),
            File(ref mut file) => file.consume(amt),
        }
    }
}

/// A result of `Multipart::save()`.
#[derive(Debug)]
pub struct Entries {
    /// The fields of the multipart request, mapped by field name -> value.
    ///
    /// A field name may have multiple actual fields associated with it, but the most
    /// common case is a single field.
    ///
    /// Each vector is guaranteed not to be empty unless externally modified.
    // Even though individual fields might only have one entry, it's better to limit the
    // size of a value type in `HashMap` to improve cache efficiency in lookups.
        pub fields: HashMap<Arc<str>, Vec<SavedField>>,
    /// The directory that the entries in `fields` were saved into.
    pub save_dir: SaveDir,
    fields_count: u32,
}

impl Entries {
    /// Create a new `Entries` with the given `SaveDir`
    pub fn new(save_dir: SaveDir) -> Self {
        Entries {
            fields: HashMap::new(),
            save_dir,
            fields_count: 0,
        }
    }

    /// Returns `true` if `fields` is empty, `false` otherwise.
    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }

    /// The number of actual fields contained within this `Entries`.
    ///
    /// Effectively `self.fields.values().map(Vec::len).sum()` but maintained separately.
    ///
    /// ## Note
    /// This will be incorrect if `fields` is modified externally. Call `recount_fields()`
    /// to get the correct count.
    pub fn fields_count(&self) -> u32 {
        self.fields_count
    }

    /// Sum the number of fields in this `Entries` and then return the updated value.
    pub fn recount_fields(&mut self) -> u32 {
        let fields_count = self.fields.values().map(Vec::len).sum();
        // saturating cast
        self.fields_count = cmp::min(u32::MAX as usize, fields_count) as u32;
        self.fields_count
    }

    fn push_field(&mut self, mut headers: FieldHeaders, data: SavedData) {
        use std::collections::hash_map::Entry::*;

        match self.fields.entry(headers.name.clone()) {
            Vacant(vacant) => { vacant.insert(vec![SavedField { headers, data }]); },
            Occupied(occupied) => {
                // dedup the field name by reusing the key's `Arc`
                headers.name = occupied.key().clone();
                occupied.into_mut().push({ SavedField { headers, data }});
            },
        }

        self.fields_count = self.fields_count.saturating_add(1);
    }

    /// Print all fields and their contents to stdout. Mostly for testing purposes.
    pub fn print_debug(&self) -> io::Result<()> {
        let stdout = io::stdout();
        let stdout_lock = stdout.lock();
        self.write_debug(stdout_lock)
    }

    /// Write all fields and their contents to the given output. Mostly for testing purposes.
    pub fn write_debug<W: Write>(&self, mut writer: W) -> io::Result<()> {
        for (name, entries) in &self.fields {
            writeln!(writer, "Field {:?} has {} entries:", name, entries.len())?;

            for (idx, field) in entries.iter().enumerate() {
                let mut data = field.data.readable()?;
                let headers = &field.headers;
                writeln!(writer, "{}: {:?} ({:?}):", idx, headers.filename, headers.content_type)?;
                io::copy(&mut data, &mut writer)?;
            }
        }

        Ok(())
    }
}

/// The save directory for `Entries`. May be temporary (delete-on-drop) or permanent.
#[derive(Debug)]
pub enum SaveDir {
    /// This directory is temporary and will be deleted, along with its contents, when this wrapper
    /// is dropped.
    Temp(TempDir),
    /// This directory is permanent and will be left on the filesystem when this wrapper is dropped.
    ///
    /// **N.B.** If this directory is in the OS temporary directory then it may still be
    /// deleted at any time.
    Perm(PathBuf),
}

impl SaveDir {
    /// Get the path of this directory, either temporary or permanent.
    pub fn as_path(&self) -> &Path {
        use self::SaveDir::*;
        match *self {
            Temp(ref tempdir) => tempdir.path(),
            Perm(ref pathbuf) => &*pathbuf,
        }
    }

    /// Returns `true` if this is a temporary directory which will be deleted on-drop.
    pub fn is_temporary(&self) -> bool {
        use self::SaveDir::*;
        match *self {
            Temp(_) => true,
            Perm(_) => false,
        }
    }

    /// Unwrap the `PathBuf` from `self`; if this is a temporary directory,
    /// it will be converted to a permanent one.
    pub fn into_path(self) -> PathBuf {
        use self::SaveDir::*;

        match self {
            Temp(tempdir) => tempdir.into_path(),
            Perm(pathbuf) => pathbuf,
        }
    }

    /// If this `SaveDir` is temporary, convert it to permanent.
    /// This is a no-op if it already is permanent.
    ///
    /// ### Warning: Potential Data Loss
    /// Even though this will prevent deletion on-drop, the temporary folder on most OSes
    /// (where this directory is created by default) can be automatically cleared by the OS at any
    /// time, usually on reboot or when free space is low.
    ///
    /// It is recommended that you relocate the files from a request which you want to keep to a
    /// permanent folder on the filesystem.
    pub fn keep(&mut self) {
        use self::SaveDir::*;
        *self = match mem::replace(self, Perm(PathBuf::new())) {
            Temp(tempdir) => Perm(tempdir.into_path()),
            old_self => old_self,
        };
    }

    /// Delete this directory and its contents, regardless of its permanence.
    ///
    /// ### Warning: Potential Data Loss
    /// This is very likely irreversible, depending on the OS implementation.
    ///
    /// Files deleted programmatically are deleted directly from disk, as compared to most file
    /// manager applications which use a staging area from which deleted files can be safely
    /// recovered (i.e. Windows' Recycle Bin, OS X's Trash Can, etc.).
    pub fn delete(self) -> io::Result<()> {
        use self::SaveDir::*;
        match self {
            Temp(tempdir) => tempdir.close(),
            Perm(pathbuf) => fs::remove_dir_all(&pathbuf),
        }
    }
}

impl AsRef<Path> for SaveDir {
    fn as_ref(&self) -> &Path {
        self.as_path()
    }
}

/// The reason the save operation quit partway through.
#[derive(Debug)]
pub enum PartialReason {
    /// The count limit for files in the request was hit.
    ///
    /// The associated file has not been saved to the filesystem.
    CountLimit,
    /// The size limit for an individual file was hit.
    ///
    /// The file was partially written to the filesystem.
    SizeLimit,
    /// An error occurred during the operation.
    IoError(io::Error),
    /// An error returned from validating a field as UTF-8 due to `SaveBuilder::force_text()`
    Utf8Error(str::Utf8Error),
}

impl From<io::Error> for PartialReason {
    fn from(e: io::Error) -> Self {
        IoError(e)
    }
}

impl From<str::Utf8Error> for PartialReason {
    fn from(e: str::Utf8Error) -> Self {
        Utf8Error(e)
    }
}

impl PartialReason {
    /// Return `io::Error` in the `IoError` case or panic otherwise.
    pub fn unwrap_err(self) -> io::Error {
        self.expect_err("`PartialReason` was not `IoError`")
    }

    /// Return `io::Error` in the `IoError` case or panic with the given
    /// message otherwise.
    pub fn expect_err(self, msg: &str) -> io::Error {
        match self {
            PartialReason::IoError(e) => e,
            _ => panic!("{}: {:?}", msg, self),
        }
    }
}

/// The field that was being read when the save operation quit.
///
/// May be partially saved to the filesystem if `dest` is `Some`.
#[derive(Debug)]
pub struct PartialSavedField<M: ReadEntry> {
    /// The field that was being read.
    ///
    /// May be partially read if `dest` is `Some`.
    pub source: MultipartField<M>,
    /// The data from the saving operation, if it got that far.
    pub dest: Option<SavedData>,
}

/// The partial result type for `Multipart::save*()`.
///
/// Contains the successfully saved entries as well as the partially
/// saved file that was in the process of being read when the error occurred,
/// if applicable.
#[derive(Debug)]
pub struct PartialEntries<M: ReadEntry> {
    /// The entries that were saved successfully.
    pub entries: Entries,
    /// The field that was in the process of being read. `None` if the error
    /// occurred between entries.
    pub partial: Option<PartialSavedField<M>>,
}

/// Discards `partial`
impl<M: ReadEntry> Into<Entries> for PartialEntries<M> {
    fn into(self) -> Entries {
        self.entries
    }
}

impl<M: ReadEntry> PartialEntries<M> {
    /// If `partial` is present and contains a `SavedFile` then just
    /// add it to the `Entries` instance and return it.
    ///
    /// Otherwise, returns `self.entries`
    pub fn keep_partial(mut self) -> Entries {
        if let Some(partial) = self.partial {
            if let Some(saved) = partial.dest {
                self.entries.push_field(partial.source.headers, saved);
            }
        }

        self.entries
    }
}

/// The ternary result type used for the `SaveBuilder<_>` API.
#[derive(Debug)]
pub enum SaveResult<Success, Partial> {
    /// The operation was a total success. Contained is the complete result.
    Full(Success),
    /// The operation quit partway through. Included is the partial
    /// result along with the reason.
    Partial(Partial, PartialReason),
    /// An error occurred at the start of the operation, before anything was done.
    Error(io::Error),
}

/// Shorthand result for methods that return `Entries`
pub type EntriesSaveResult<M> = SaveResult<Entries, PartialEntries<M>>;

/// Shorthand result for methods that return `FieldData`s.
///
/// The `MultipartData` is not provided here because it is not necessary to return
/// a borrow when the owned version is probably in the same scope. This hopefully
/// saves some headache with the borrow-checker.
pub type FieldSaveResult = SaveResult<SavedData, SavedData>;

impl<M: ReadEntry> EntriesSaveResult<M> {
    /// Take the `Entries` from `self`, if applicable, and discarding
    /// the error, if any.
    pub fn into_entries(self) -> Option<Entries> {
        match self {
            Full(entries) | Partial(PartialEntries { entries, .. }, _) => Some(entries),
            Error(_) => None,
        }
    }
}

impl<S, P> SaveResult<S, P> where P: Into<S> {
    /// Convert `self` to `Option<S>`; there may still have been an error.
    pub fn okish(self) -> Option<S> {
        self.into_opt_both().0
    }

    /// Map the `Full` or `Partial` values to a new type, retaining the reason
    /// in the `Partial` case.
    pub fn map<T, Map>(self, map: Map) -> SaveResult<T, T> where Map: FnOnce(S) -> T {
        match self {
            Full(full) => Full(map(full)),
            Partial(partial, reason) => Partial(map(partial.into()), reason),
            Error(e) => Error(e),
        }
    }

    /// Decompose `self` to `(Option<S>, Option<io::Error>)`
    pub fn into_opt_both(self) -> (Option<S>, Option<io::Error>) {
        match self {
            Full(full)  => (Some(full), None),
            Partial(partial, IoError(e)) => (Some(partial.into()), Some(e)),
            Partial(partial, _) => (Some(partial.into()), None),
            Error(error) => (None, Some(error)),
        }
    }

    /// Map `self` to an `io::Result`, discarding the error in the `Partial` case.
    pub fn into_result(self) -> io::Result<S> {
        match self {
            Full(entries) => Ok(entries),
            Partial(partial, _) => Ok(partial.into()),
            Error(error) => Err(error),
        }
    }

    /// Pessimistic version of `into_result()` which will return an error even
    /// for the `Partial` case.
    ///
    /// ### Note: Possible Storage Leak
    /// It's generally not a good idea to ignore the `Partial` case, as there may still be a
    /// partially written file on-disk. If you're not using a temporary directory
    /// (OS-managed or via `TempDir`) then partially written files will remain on-disk until
    /// explicitly removed which could result in excessive disk usage if not monitored closely.
    pub fn into_result_strict(self) -> io::Result<S> {
        match self {
            Full(entries) => Ok(entries),
            Partial(_, PartialReason::IoError(e)) | Error(e) => Err(e),
            Partial(partial, _) => Ok(partial.into()),
        }
    }
}

fn create_dir_all(path: &Path) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
    } else {
        // RFC: return an error instead?
        warn!("Attempting to save file in what looks like a root directory. File path: {:?}", path);
        Ok(())
    }
}

fn try_copy_limited<R: BufRead, Wb: FnMut(&[u8]) -> SaveResult<usize, usize>>(src: R, mut with_buf: Wb, limit: u64) -> SaveResult<u64, u64> {
    let mut copied = 0u64;
    try_read_buf(src, |buf| {
        let new_copied = copied.saturating_add(buf.len() as u64);
        if new_copied > limit { return Partial(0, PartialReason::SizeLimit) }
        copied = new_copied;

        with_buf(buf)
    })
}

fn try_read_buf<R: BufRead, Wb: FnMut(&[u8]) -> SaveResult<usize, usize>>(mut src: R, mut with_buf: Wb) -> SaveResult<u64, u64> {
    let mut total_copied = 0u64;

    macro_rules! try_here (
        ($try:expr) => (
            match $try {
                Ok(val) => val,
                Err(ref e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return if total_copied == 0 { Error(e) }
                                 else { Partial(total_copied, e.into()) },
            }
        )
    );

    loop {
        let res = {
            let buf = try_here!(src.fill_buf());
            if buf.is_empty() { break; }
            with_buf(buf)
        };

        match res {
            Full(copied) => { src.consume(copied); total_copied += copied as u64; }
            Partial(copied, reason) => {
                src.consume(copied); total_copied += copied as u64;
                return Partial(total_copied, reason);
            },
            Error(err) => {
                return Partial(total_copied, err.into());
            }
        }
    }

    Full(total_copied)
}

fn try_write_all<W: Write>(mut buf: &[u8], mut dest: W) -> SaveResult<usize, usize> {
    let mut total_copied = 0;

    macro_rules! try_here (
        ($try:expr) => (
            match $try {
                Ok(val) => val,
                Err(ref e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return if total_copied == 0 { Error(e) }
                                 else { Partial(total_copied, e.into()) },
            }
        )
    );

    while !buf.is_empty() {
        match try_here!(dest.write(buf)) {
            0 => try_here!(Err(io::Error::new(io::ErrorKind::WriteZero,
                                          "failed to write whole buffer"))),
            copied => {
                buf = &buf[copied..];
                total_copied += copied;
            },
        }
    }

    Full(total_copied)
}
