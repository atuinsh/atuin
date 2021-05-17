use std::cmp;
use std::fmt;
use std::iter::FromIterator;
use std::ops::{self, Range};
use std::result;

use bstr::{BString, ByteSlice};
use serde::de::Deserialize;

use crate::deserializer::deserialize_byte_record;
use crate::error::{new_utf8_error, Result, Utf8Error};
use crate::string_record::StringRecord;

/// A single CSV record stored as raw bytes.
///
/// A byte record permits reading or writing CSV rows that are not UTF-8.
/// In general, you should prefer using a
/// [`StringRecord`](struct.StringRecord.html)
/// since it is more ergonomic, but a `ByteRecord` is provided in case you need
/// it.
///
/// If you are using the Serde (de)serialization APIs, then you probably never
/// need to interact with a `ByteRecord` or a `StringRecord`. However, there
/// are some circumstances in which you might need to use a raw record type
/// while still using Serde. For example, if you need to deserialize possibly
/// invalid UTF-8 fields, then you'll need to first read your record into a
/// `ByteRecord`, and then use `ByteRecord::deserialize` to run Serde. Another
/// reason for using the raw record deserialization APIs is if you're using
/// Serde to read into borrowed data such as a `&'a str` or a `&'a [u8]`.
///
/// Two `ByteRecord`s are compared on the basis of their field data. Any
/// position information associated with the records is ignored.
#[derive(Clone, Eq)]
pub struct ByteRecord(Box<ByteRecordInner>);

impl PartialEq for ByteRecord {
    fn eq(&self, other: &ByteRecord) -> bool {
        if self.len() != other.len() {
            return false;
        }
        self.iter().zip(other.iter()).all(|e| e.0 == e.1)
    }
}

impl<T: AsRef<[u8]>> PartialEq<Vec<T>> for ByteRecord {
    fn eq(&self, other: &Vec<T>) -> bool {
        self.iter_eq(other)
    }
}

impl<'a, T: AsRef<[u8]>> PartialEq<Vec<T>> for &'a ByteRecord {
    fn eq(&self, other: &Vec<T>) -> bool {
        self.iter_eq(other)
    }
}

impl<T: AsRef<[u8]>> PartialEq<[T]> for ByteRecord {
    fn eq(&self, other: &[T]) -> bool {
        self.iter_eq(other)
    }
}

impl<'a, T: AsRef<[u8]>> PartialEq<[T]> for &'a ByteRecord {
    fn eq(&self, other: &[T]) -> bool {
        self.iter_eq(other)
    }
}

impl fmt::Debug for ByteRecord {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let mut fields = vec![];
        for field in self {
            fields.push(BString::from(field.to_vec()));
        }
        write!(f, "ByteRecord({:?})", fields)
    }
}

/// The inner portion of a byte record.
///
/// We use this memory layout so that moving a `ByteRecord` only requires
/// moving a single pointer. The optimization is dubious at best, but does
/// seem to result in slightly better numbers in microbenchmarks. Methinks this
/// may heavily depend on the underlying allocator.
#[derive(Clone, Debug, Eq, PartialEq)]
struct ByteRecordInner {
    /// The position of this byte record.
    pos: Option<Position>,
    /// All fields in this record, stored contiguously.
    fields: Vec<u8>,
    /// The number of and location of each field in this record.
    bounds: Bounds,
}

impl Default for ByteRecord {
    #[inline]
    fn default() -> ByteRecord {
        ByteRecord::new()
    }
}

impl ByteRecord {
    /// Create a new empty `ByteRecord`.
    ///
    /// Note that you may find the `ByteRecord::from` constructor more
    /// convenient, which is provided by an impl on the `From` trait.
    ///
    /// # Example: create an empty record
    ///
    /// ```
    /// use csv::ByteRecord;
    ///
    /// let record = ByteRecord::new();
    /// assert_eq!(record.len(), 0);
    /// ```
    ///
    /// # Example: initialize a record from a `Vec`
    ///
    /// ```
    /// use csv::ByteRecord;
    ///
    /// let record = ByteRecord::from(vec!["a", "b", "c"]);
    /// assert_eq!(record.len(), 3);
    /// ```
    #[inline]
    pub fn new() -> ByteRecord {
        ByteRecord::with_capacity(0, 0)
    }

    /// Create a new empty `ByteRecord` with the given capacity settings.
    ///
    /// `buffer` refers to the capacity of the buffer used to store the
    /// actual row contents. `fields` refers to the number of fields one
    /// might expect to store.
    #[inline]
    pub fn with_capacity(buffer: usize, fields: usize) -> ByteRecord {
        ByteRecord(Box::new(ByteRecordInner {
            pos: None,
            fields: vec![0; buffer],
            bounds: Bounds::with_capacity(fields),
        }))
    }

    /// Deserialize this record.
    ///
    /// The `D` type parameter refers to the type that this record should be
    /// deserialized into. The `'de` lifetime refers to the lifetime of the
    /// `ByteRecord`. The `'de` lifetime permits deserializing into structs
    /// that borrow field data from this record.
    ///
    /// An optional `headers` parameter permits deserializing into a struct
    /// based on its field names (corresponding to header values) rather than
    /// the order in which the fields are defined.
    ///
    /// # Example: without headers
    ///
    /// This shows how to deserialize a single row into a struct based on the
    /// order in which fields occur. This example also shows how to borrow
    /// fields from the `ByteRecord`, which results in zero allocation
    /// deserialization.
    ///
    /// ```
    /// use std::error::Error;
    ///
    /// use csv::ByteRecord;
    /// use serde::Deserialize;
    ///
    /// #[derive(Deserialize)]
    /// struct Row<'a> {
    ///     city: &'a str,
    ///     country: &'a str,
    ///     population: u64,
    /// }
    ///
    /// # fn main() { example().unwrap() }
    /// fn example() -> Result<(), Box<dyn Error>> {
    ///     let record = ByteRecord::from(vec![
    ///         "Boston", "United States", "4628910",
    ///     ]);
    ///
    ///     let row: Row = record.deserialize(None)?;
    ///     assert_eq!(row.city, "Boston");
    ///     assert_eq!(row.country, "United States");
    ///     assert_eq!(row.population, 4628910);
    ///     Ok(())
    /// }
    /// ```
    ///
    /// # Example: with headers
    ///
    /// This example is like the previous one, but shows how to deserialize
    /// into a struct based on the struct's field names. For this to work,
    /// you must provide a header row.
    ///
    /// This example also shows that you can deserialize into owned data
    /// types (e.g., `String`) instead of borrowed data types (e.g., `&str`).
    ///
    /// ```
    /// use std::error::Error;
    ///
    /// use csv::ByteRecord;
    /// use serde::Deserialize;
    ///
    /// #[derive(Deserialize)]
    /// struct Row {
    ///     city: String,
    ///     country: String,
    ///     population: u64,
    /// }
    ///
    /// # fn main() { example().unwrap() }
    /// fn example() -> Result<(), Box<dyn Error>> {
    ///     // Notice that the fields are not in the same order
    ///     // as the fields in the struct!
    ///     let header = ByteRecord::from(vec![
    ///         "country", "city", "population",
    ///     ]);
    ///     let record = ByteRecord::from(vec![
    ///         "United States", "Boston", "4628910",
    ///     ]);
    ///
    ///     let row: Row = record.deserialize(Some(&header))?;
    ///     assert_eq!(row.city, "Boston");
    ///     assert_eq!(row.country, "United States");
    ///     assert_eq!(row.population, 4628910);
    ///     Ok(())
    /// }
    /// ```
    pub fn deserialize<'de, D: Deserialize<'de>>(
        &'de self,
        headers: Option<&'de ByteRecord>,
    ) -> Result<D> {
        deserialize_byte_record(self, headers)
    }

    /// Returns an iterator over all fields in this record.
    ///
    /// # Example
    ///
    /// This example shows how to iterate over each field in a `ByteRecord`.
    ///
    /// ```
    /// use csv::ByteRecord;
    ///
    /// let record = ByteRecord::from(vec!["a", "b", "c"]);
    /// for field in record.iter() {
    ///     assert!(field == b"a" || field == b"b" || field == b"c");
    /// }
    /// ```
    #[inline]
    pub fn iter(&self) -> ByteRecordIter {
        self.into_iter()
    }

    /// Return the field at index `i`.
    ///
    /// If no field at index `i` exists, then this returns `None`.
    ///
    /// # Example
    ///
    /// ```
    /// use csv::ByteRecord;
    ///
    /// let record = ByteRecord::from(vec!["a", "b", "c"]);
    /// assert_eq!(record.get(1), Some(&b"b"[..]));
    /// assert_eq!(record.get(3), None);
    /// ```
    #[inline]
    pub fn get(&self, i: usize) -> Option<&[u8]> {
        self.0.bounds.get(i).map(|range| &self.0.fields[range])
    }

    /// Returns true if and only if this record is empty.
    ///
    /// # Example
    ///
    /// ```
    /// use csv::ByteRecord;
    ///
    /// assert!(ByteRecord::new().is_empty());
    /// ```
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Returns the number of fields in this record.
    ///
    /// # Example
    ///
    /// ```
    /// use csv::ByteRecord;
    ///
    /// let record = ByteRecord::from(vec!["a", "b", "c"]);
    /// assert_eq!(record.len(), 3);
    /// ```
    #[inline]
    pub fn len(&self) -> usize {
        self.0.bounds.len()
    }

    /// Truncate this record to `n` fields.
    ///
    /// If `n` is greater than the number of fields in this record, then this
    /// has no effect.
    ///
    /// # Example
    ///
    /// ```
    /// use csv::ByteRecord;
    ///
    /// let mut record = ByteRecord::from(vec!["a", "b", "c"]);
    /// assert_eq!(record.len(), 3);
    /// record.truncate(1);
    /// assert_eq!(record.len(), 1);
    /// assert_eq!(record, vec!["a"]);
    /// ```
    #[inline]
    pub fn truncate(&mut self, n: usize) {
        if n <= self.len() {
            self.0.bounds.len = n;
        }
    }

    /// Clear this record so that it has zero fields.
    ///
    /// This is equivalent to calling `truncate(0)`.
    ///
    /// Note that it is not necessary to clear the record to reuse it with
    /// the CSV reader.
    ///
    /// # Example
    ///
    /// ```
    /// use csv::ByteRecord;
    ///
    /// let mut record = ByteRecord::from(vec!["a", "b", "c"]);
    /// assert_eq!(record.len(), 3);
    /// record.clear();
    /// assert_eq!(record.len(), 0);
    /// ```
    #[inline]
    pub fn clear(&mut self) {
        self.truncate(0);
    }

    /// Trim the fields of this record so that leading and trailing whitespace
    /// is removed.
    ///
    /// This method uses the ASCII definition of whitespace. That is, only
    /// bytes in the class `[\t\n\v\f\r ]` are trimmed.
    ///
    /// # Example
    ///
    /// ```
    /// use csv::ByteRecord;
    ///
    /// let mut record = ByteRecord::from(vec![
    ///     "  ", "\tfoo", "bar  ", "b a z",
    /// ]);
    /// record.trim();
    /// assert_eq!(record, vec!["", "foo", "bar", "b a z"]);
    /// ```
    pub fn trim(&mut self) {
        let length = self.len();
        if length == 0 {
            return;
        }
        // TODO: We could likely do this in place, but for now, we allocate.
        let mut trimmed =
            ByteRecord::with_capacity(self.as_slice().len(), self.len());
        trimmed.set_position(self.position().cloned());
        for field in &*self {
            trimmed.push_field(field.trim());
        }
        *self = trimmed;
    }

    /// Add a new field to this record.
    ///
    /// # Example
    ///
    /// ```
    /// use csv::ByteRecord;
    ///
    /// let mut record = ByteRecord::new();
    /// record.push_field(b"foo");
    /// assert_eq!(&record[0], b"foo");
    /// ```
    #[inline]
    pub fn push_field(&mut self, field: &[u8]) {
        let (s, e) = (self.0.bounds.end(), self.0.bounds.end() + field.len());
        while e > self.0.fields.len() {
            self.expand_fields();
        }
        self.0.fields[s..e].copy_from_slice(field);
        self.0.bounds.add(e);
    }

    /// Return the position of this record, if available.
    ///
    /// # Example
    ///
    /// ```
    /// use std::error::Error;
    ///
    /// use csv::{ByteRecord, ReaderBuilder};
    ///
    /// # fn main() { example().unwrap(); }
    /// fn example() -> Result<(), Box<dyn Error>> {
    ///     let mut record = ByteRecord::new();
    ///     let mut rdr = ReaderBuilder::new()
    ///         .has_headers(false)
    ///         .from_reader("a,b,c\nx,y,z".as_bytes());
    ///
    ///     assert!(rdr.read_byte_record(&mut record)?);
    ///     {
    ///         let pos = record.position().expect("a record position");
    ///         assert_eq!(pos.byte(), 0);
    ///         assert_eq!(pos.line(), 1);
    ///         assert_eq!(pos.record(), 0);
    ///     }
    ///
    ///     assert!(rdr.read_byte_record(&mut record)?);
    ///     {
    ///         let pos = record.position().expect("a record position");
    ///         assert_eq!(pos.byte(), 6);
    ///         assert_eq!(pos.line(), 2);
    ///         assert_eq!(pos.record(), 1);
    ///     }
    ///
    ///     // Finish the CSV reader for good measure.
    ///     assert!(!rdr.read_byte_record(&mut record)?);
    ///     Ok(())
    /// }
    /// ```
    #[inline]
    pub fn position(&self) -> Option<&Position> {
        self.0.pos.as_ref()
    }

    /// Set the position of this record.
    ///
    /// # Example
    ///
    /// ```
    /// use csv::{ByteRecord, Position};
    ///
    /// let mut record = ByteRecord::from(vec!["a", "b", "c"]);
    /// let mut pos = Position::new();
    /// pos.set_byte(100);
    /// pos.set_line(4);
    /// pos.set_record(2);
    ///
    /// record.set_position(Some(pos.clone()));
    /// assert_eq!(record.position(), Some(&pos));
    /// ```
    #[inline]
    pub fn set_position(&mut self, pos: Option<Position>) {
        self.0.pos = pos;
    }

    /// Return the start and end position of a field in this record.
    ///
    /// If no such field exists at the given index, then return `None`.
    ///
    /// The range returned can be used with the slice returned by `as_slice`.
    ///
    /// # Example
    ///
    /// ```
    /// use csv::ByteRecord;
    ///
    /// let record = ByteRecord::from(vec!["foo", "quux", "z"]);
    /// let range = record.range(1).expect("a record range");
    /// assert_eq!(&record.as_slice()[range], &b"quux"[..]);
    /// ```
    #[inline]
    pub fn range(&self, i: usize) -> Option<Range<usize>> {
        self.0.bounds.get(i)
    }

    /// Return the entire row as a single byte slice. The slice returned stores
    /// all fields contiguously. The boundaries of each field can be determined
    /// via the `range` method.
    ///
    /// # Example
    ///
    /// ```
    /// use csv::ByteRecord;
    ///
    /// let record = ByteRecord::from(vec!["foo", "quux", "z"]);
    /// assert_eq!(record.as_slice(), &b"fooquuxz"[..]);
    /// ```
    #[inline]
    pub fn as_slice(&self) -> &[u8] {
        &self.0.fields[..self.0.bounds.end()]
    }

    /// Clone this record, but only copy `fields` up to the end of bounds. This
    /// is useful when one wants to copy a record, but not necessarily any
    /// excess capacity in that record.
    #[inline]
    pub(crate) fn clone_truncated(&self) -> ByteRecord {
        let mut br = ByteRecord::new();
        br.0.pos = self.0.pos.clone();
        br.0.bounds = self.0.bounds.clone();
        br.0.fields = self.0.fields[..self.0.bounds.end()].to_vec();
        br
    }

    /// Retrieve the underlying parts of a byte record.
    #[inline]
    pub(crate) fn as_parts(&mut self) -> (&mut Vec<u8>, &mut Vec<usize>) {
        let inner = &mut *self.0;
        (&mut inner.fields, &mut inner.bounds.ends)
    }

    /// Set the number of fields in the given record record.
    #[inline]
    pub(crate) fn set_len(&mut self, len: usize) {
        self.0.bounds.len = len;
    }

    /// Expand the capacity for storing fields.
    #[inline]
    pub(crate) fn expand_fields(&mut self) {
        let new_len = self.0.fields.len().checked_mul(2).unwrap();
        self.0.fields.resize(cmp::max(4, new_len), 0);
    }

    /// Expand the capacity for storing field ending positions.
    #[inline]
    pub(crate) fn expand_ends(&mut self) {
        self.0.bounds.expand();
    }

    /// Validate the given record as UTF-8.
    ///
    /// If it's not UTF-8, return an error.
    #[inline]
    pub(crate) fn validate(&self) -> result::Result<(), Utf8Error> {
        // If the entire buffer is ASCII, then we have nothing to fear.
        if self.0.fields[..self.0.bounds.end()].is_ascii() {
            return Ok(());
        }
        // Otherwise, we must check each field individually to ensure that
        // it's valid UTF-8.
        for (i, field) in self.iter().enumerate() {
            if let Err(err) = field.to_str() {
                return Err(new_utf8_error(i, err.valid_up_to()));
            }
        }
        Ok(())
    }

    /// Compare the given byte record with the iterator of fields for equality.
    pub(crate) fn iter_eq<I, T>(&self, other: I) -> bool
    where
        I: IntoIterator<Item = T>,
        T: AsRef<[u8]>,
    {
        let mut it_record = self.iter();
        let mut it_other = other.into_iter();
        loop {
            match (it_record.next(), it_other.next()) {
                (None, None) => return true,
                (None, Some(_)) | (Some(_), None) => return false,
                (Some(x), Some(y)) => {
                    if x != y.as_ref() {
                        return false;
                    }
                }
            }
        }
    }
}

/// A position in CSV data.
///
/// A position is used to report errors in CSV data. All positions include the
/// byte offset, line number and record index at which the error occurred.
///
/// Byte offsets and record indices start at `0`. Line numbers start at `1`.
///
/// A CSV reader will automatically assign the position of each record.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Position {
    byte: u64,
    line: u64,
    record: u64,
}

impl Position {
    /// Returns a new position initialized to the start value.
    #[inline]
    pub fn new() -> Position {
        Position { byte: 0, line: 1, record: 0 }
    }

    /// The byte offset, starting at `0`, of this position.
    #[inline]
    pub fn byte(&self) -> u64 {
        self.byte
    }
    /// The line number, starting at `1`, of this position.
    #[inline]
    pub fn line(&self) -> u64 {
        self.line
    }
    /// The record index, starting with the first record at `0`.
    #[inline]
    pub fn record(&self) -> u64 {
        self.record
    }

    /// Set the byte offset of this position.
    #[inline]
    pub fn set_byte(&mut self, byte: u64) -> &mut Position {
        self.byte = byte;
        self
    }

    /// Set the line number of this position.
    ///
    /// If the line number is less than `1`, then this method panics.
    #[inline]
    pub fn set_line(&mut self, line: u64) -> &mut Position {
        assert!(line > 0);
        self.line = line;
        self
    }

    /// Set the record index of this position.
    #[inline]
    pub fn set_record(&mut self, record: u64) -> &mut Position {
        self.record = record;
        self
    }
}

/// The bounds of fields in a single record.
#[derive(Clone, Debug, Eq, PartialEq)]
struct Bounds {
    /// The ending index of each field.
    ends: Vec<usize>,
    /// The number of fields in this record.
    ///
    /// Technically, we could drop this field and maintain an invariant that
    /// `ends.len()` is always the number of fields, but doing that efficiently
    /// requires attention to safety. We play it safe at essentially no cost.
    len: usize,
}

impl Default for Bounds {
    #[inline]
    fn default() -> Bounds {
        Bounds::with_capacity(0)
    }
}

impl Bounds {
    /// Create a new set of bounds with the given capacity for storing the
    /// ends of fields.
    #[inline]
    fn with_capacity(capacity: usize) -> Bounds {
        Bounds { ends: vec![0; capacity], len: 0 }
    }

    /// Returns the bounds of field `i`.
    #[inline]
    fn get(&self, i: usize) -> Option<Range<usize>> {
        if i >= self.len {
            return None;
        }
        let end = match self.ends.get(i) {
            None => return None,
            Some(&end) => end,
        };
        let start = match i.checked_sub(1).and_then(|i| self.ends.get(i)) {
            None => 0,
            Some(&start) => start,
        };
        Some(ops::Range { start: start, end: end })
    }

    /// Returns a slice of ending positions of all fields.
    #[inline]
    fn ends(&self) -> &[usize] {
        &self.ends[..self.len]
    }

    /// Return the last position of the last field.
    ///
    /// If there are no fields, this returns `0`.
    #[inline]
    fn end(&self) -> usize {
        self.ends().last().map(|&i| i).unwrap_or(0)
    }

    /// Returns the number of fields in these bounds.
    #[inline]
    fn len(&self) -> usize {
        self.len
    }

    /// Expand the capacity for storing field ending positions.
    #[inline]
    fn expand(&mut self) {
        let new_len = self.ends.len().checked_mul(2).unwrap();
        self.ends.resize(cmp::max(4, new_len), 0);
    }

    /// Add a new field with the given ending position.
    #[inline]
    fn add(&mut self, pos: usize) {
        if self.len >= self.ends.len() {
            self.expand();
        }
        self.ends[self.len] = pos;
        self.len += 1;
    }
}

impl ops::Index<usize> for ByteRecord {
    type Output = [u8];
    #[inline]
    fn index(&self, i: usize) -> &[u8] {
        self.get(i).unwrap()
    }
}

impl From<StringRecord> for ByteRecord {
    #[inline]
    fn from(record: StringRecord) -> ByteRecord {
        record.into_byte_record()
    }
}

impl<T: AsRef<[u8]>> From<Vec<T>> for ByteRecord {
    #[inline]
    fn from(xs: Vec<T>) -> ByteRecord {
        ByteRecord::from_iter(&xs)
    }
}

impl<'a, T: AsRef<[u8]>> From<&'a [T]> for ByteRecord {
    #[inline]
    fn from(xs: &'a [T]) -> ByteRecord {
        ByteRecord::from_iter(xs)
    }
}

impl<T: AsRef<[u8]>> FromIterator<T> for ByteRecord {
    #[inline]
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> ByteRecord {
        let mut record = ByteRecord::new();
        record.extend(iter);
        record
    }
}

impl<T: AsRef<[u8]>> Extend<T> for ByteRecord {
    #[inline]
    fn extend<I: IntoIterator<Item = T>>(&mut self, iter: I) {
        for x in iter {
            self.push_field(x.as_ref());
        }
    }
}

/// A double-ended iterator over the fields in a byte record.
///
/// The `'r` lifetime variable refers to the lifetime of the `ByteRecord` that
/// is being iterated over.
#[derive(Clone)]
pub struct ByteRecordIter<'r> {
    /// The record we are iterating over.
    r: &'r ByteRecord,
    /// The starting index of the previous field. (For reverse iteration.)
    last_start: usize,
    /// The ending index of the previous field. (For forward iteration.)
    last_end: usize,
    /// The index of forward iteration.
    i_forward: usize,
    /// The index of reverse iteration.
    i_reverse: usize,
}

impl<'r> IntoIterator for &'r ByteRecord {
    type IntoIter = ByteRecordIter<'r>;
    type Item = &'r [u8];

    #[inline]
    fn into_iter(self) -> ByteRecordIter<'r> {
        ByteRecordIter {
            r: self,
            last_start: self.as_slice().len(),
            last_end: 0,
            i_forward: 0,
            i_reverse: self.len(),
        }
    }
}

impl<'r> ExactSizeIterator for ByteRecordIter<'r> {}

impl<'r> Iterator for ByteRecordIter<'r> {
    type Item = &'r [u8];

    #[inline]
    fn next(&mut self) -> Option<&'r [u8]> {
        if self.i_forward == self.i_reverse {
            None
        } else {
            let start = self.last_end;
            let end = self.r.0.bounds.ends()[self.i_forward];
            self.i_forward += 1;
            self.last_end = end;
            Some(&self.r.0.fields[start..end])
        }
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        let x = self.i_reverse - self.i_forward;
        (x, Some(x))
    }

    #[inline]
    fn count(self) -> usize {
        self.len()
    }
}

impl<'r> DoubleEndedIterator for ByteRecordIter<'r> {
    #[inline]
    fn next_back(&mut self) -> Option<&'r [u8]> {
        if self.i_forward == self.i_reverse {
            None
        } else {
            self.i_reverse -= 1;
            let start = self
                .i_reverse
                .checked_sub(1)
                .map(|i| self.r.0.bounds.ends()[i])
                .unwrap_or(0);
            let end = self.last_start;
            self.last_start = start;
            Some(&self.r.0.fields[start..end])
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::string_record::StringRecord;

    use super::ByteRecord;

    fn b(s: &str) -> &[u8] {
        s.as_bytes()
    }

    #[test]
    fn record_1() {
        let mut rec = ByteRecord::new();
        rec.push_field(b"foo");

        assert_eq!(rec.len(), 1);
        assert_eq!(rec.get(0), Some(b("foo")));
        assert_eq!(rec.get(1), None);
        assert_eq!(rec.get(2), None);
    }

    #[test]
    fn record_2() {
        let mut rec = ByteRecord::new();
        rec.push_field(b"foo");
        rec.push_field(b"quux");

        assert_eq!(rec.len(), 2);
        assert_eq!(rec.get(0), Some(b("foo")));
        assert_eq!(rec.get(1), Some(b("quux")));
        assert_eq!(rec.get(2), None);
        assert_eq!(rec.get(3), None);
    }

    #[test]
    fn empty_record() {
        let rec = ByteRecord::new();

        assert_eq!(rec.len(), 0);
        assert_eq!(rec.get(0), None);
        assert_eq!(rec.get(1), None);
    }

    #[test]
    fn trim_whitespace_only() {
        let mut rec = ByteRecord::from(vec![b" \t\n\r\x0c"]);
        rec.trim();
        assert_eq!(rec.get(0), Some(b("")));
    }

    #[test]
    fn trim_front() {
        let mut rec = ByteRecord::from(vec![b" abc"]);
        rec.trim();
        assert_eq!(rec.get(0), Some(b("abc")));

        let mut rec = ByteRecord::from(vec![b(" abc"), b("  xyz")]);
        rec.trim();
        assert_eq!(rec.get(0), Some(b("abc")));
        assert_eq!(rec.get(1), Some(b("xyz")));
    }

    #[test]
    fn trim_back() {
        let mut rec = ByteRecord::from(vec![b"abc "]);
        rec.trim();
        assert_eq!(rec.get(0), Some(b("abc")));

        let mut rec = ByteRecord::from(vec![b("abc "), b("xyz  ")]);
        rec.trim();
        assert_eq!(rec.get(0), Some(b("abc")));
        assert_eq!(rec.get(1), Some(b("xyz")));
    }

    #[test]
    fn trim_both() {
        let mut rec = ByteRecord::from(vec![b" abc "]);
        rec.trim();
        assert_eq!(rec.get(0), Some(b("abc")));

        let mut rec = ByteRecord::from(vec![b(" abc "), b("  xyz  ")]);
        rec.trim();
        assert_eq!(rec.get(0), Some(b("abc")));
        assert_eq!(rec.get(1), Some(b("xyz")));
    }

    #[test]
    fn trim_does_not_panic_on_empty_records_1() {
        let mut rec = ByteRecord::from(vec![b""]);
        rec.trim();
        assert_eq!(rec.get(0), Some(b("")));
    }

    #[test]
    fn trim_does_not_panic_on_empty_records_2() {
        let mut rec = ByteRecord::from(vec![b"", b""]);
        rec.trim();
        assert_eq!(rec.get(0), Some(b("")));
        assert_eq!(rec.get(1), Some(b("")));
    }

    #[test]
    fn trim_does_not_panic_on_empty_records_3() {
        let mut rec = ByteRecord::new();
        rec.trim();
        assert_eq!(rec.as_slice().len(), 0);
    }

    #[test]
    fn empty_field_1() {
        let mut rec = ByteRecord::new();
        rec.push_field(b"");

        assert_eq!(rec.len(), 1);
        assert_eq!(rec.get(0), Some(b("")));
        assert_eq!(rec.get(1), None);
        assert_eq!(rec.get(2), None);
    }

    #[test]
    fn empty_field_2() {
        let mut rec = ByteRecord::new();
        rec.push_field(b"");
        rec.push_field(b"");

        assert_eq!(rec.len(), 2);
        assert_eq!(rec.get(0), Some(b("")));
        assert_eq!(rec.get(1), Some(b("")));
        assert_eq!(rec.get(2), None);
        assert_eq!(rec.get(3), None);
    }

    #[test]
    fn empty_surround_1() {
        let mut rec = ByteRecord::new();
        rec.push_field(b"foo");
        rec.push_field(b"");
        rec.push_field(b"quux");

        assert_eq!(rec.len(), 3);
        assert_eq!(rec.get(0), Some(b("foo")));
        assert_eq!(rec.get(1), Some(b("")));
        assert_eq!(rec.get(2), Some(b("quux")));
        assert_eq!(rec.get(3), None);
        assert_eq!(rec.get(4), None);
    }

    #[test]
    fn empty_surround_2() {
        let mut rec = ByteRecord::new();
        rec.push_field(b"foo");
        rec.push_field(b"");
        rec.push_field(b"quux");
        rec.push_field(b"");

        assert_eq!(rec.len(), 4);
        assert_eq!(rec.get(0), Some(b("foo")));
        assert_eq!(rec.get(1), Some(b("")));
        assert_eq!(rec.get(2), Some(b("quux")));
        assert_eq!(rec.get(3), Some(b("")));
        assert_eq!(rec.get(4), None);
        assert_eq!(rec.get(5), None);
    }

    #[test]
    fn utf8_error_1() {
        let mut rec = ByteRecord::new();
        rec.push_field(b"foo");
        rec.push_field(b"b\xFFar");

        let err = StringRecord::from_byte_record(rec).unwrap_err();
        assert_eq!(err.utf8_error().field(), 1);
        assert_eq!(err.utf8_error().valid_up_to(), 1);
    }

    #[test]
    fn utf8_error_2() {
        let mut rec = ByteRecord::new();
        rec.push_field(b"\xFF");

        let err = StringRecord::from_byte_record(rec).unwrap_err();
        assert_eq!(err.utf8_error().field(), 0);
        assert_eq!(err.utf8_error().valid_up_to(), 0);
    }

    #[test]
    fn utf8_error_3() {
        let mut rec = ByteRecord::new();
        rec.push_field(b"a\xFF");

        let err = StringRecord::from_byte_record(rec).unwrap_err();
        assert_eq!(err.utf8_error().field(), 0);
        assert_eq!(err.utf8_error().valid_up_to(), 1);
    }

    #[test]
    fn utf8_error_4() {
        let mut rec = ByteRecord::new();
        rec.push_field(b"a");
        rec.push_field(b"b");
        rec.push_field(b"c");
        rec.push_field(b"d");
        rec.push_field(b"xyz\xFF");

        let err = StringRecord::from_byte_record(rec).unwrap_err();
        assert_eq!(err.utf8_error().field(), 4);
        assert_eq!(err.utf8_error().valid_up_to(), 3);
    }

    #[test]
    fn utf8_error_5() {
        let mut rec = ByteRecord::new();
        rec.push_field(b"a");
        rec.push_field(b"b");
        rec.push_field(b"c");
        rec.push_field(b"d");
        rec.push_field(b"\xFFxyz");

        let err = StringRecord::from_byte_record(rec).unwrap_err();
        assert_eq!(err.utf8_error().field(), 4);
        assert_eq!(err.utf8_error().valid_up_to(), 0);
    }

    // This tests a tricky case where a single field on its own isn't valid
    // UTF-8, but the concatenation of all fields is.
    #[test]
    fn utf8_error_6() {
        let mut rec = ByteRecord::new();
        rec.push_field(b"a\xc9");
        rec.push_field(b"\x91b");

        let err = StringRecord::from_byte_record(rec).unwrap_err();
        assert_eq!(err.utf8_error().field(), 0);
        assert_eq!(err.utf8_error().valid_up_to(), 1);
    }

    // This tests that we can always clear a `ByteRecord` and get a guaranteed
    // successful conversion to UTF-8. This permits reusing the allocation.
    #[test]
    fn utf8_clear_ok() {
        let mut rec = ByteRecord::new();
        rec.push_field(b"\xFF");
        assert!(StringRecord::from_byte_record(rec).is_err());

        let mut rec = ByteRecord::new();
        rec.push_field(b"\xFF");
        rec.clear();
        assert!(StringRecord::from_byte_record(rec).is_ok());
    }

    #[test]
    fn iter() {
        let data = vec!["foo", "bar", "baz", "quux", "wat"];
        let rec = ByteRecord::from(&*data);
        let got: Vec<&str> =
            rec.iter().map(|x| ::std::str::from_utf8(x).unwrap()).collect();
        assert_eq!(data, got);
    }

    #[test]
    fn iter_reverse() {
        let mut data = vec!["foo", "bar", "baz", "quux", "wat"];
        let rec = ByteRecord::from(&*data);
        let got: Vec<&str> = rec
            .iter()
            .rev()
            .map(|x| ::std::str::from_utf8(x).unwrap())
            .collect();
        data.reverse();
        assert_eq!(data, got);
    }

    #[test]
    fn iter_forward_and_reverse() {
        let data = vec!["foo", "bar", "baz", "quux", "wat"];
        let rec = ByteRecord::from(data);
        let mut it = rec.iter();

        assert_eq!(it.next_back(), Some(b("wat")));
        assert_eq!(it.next(), Some(b("foo")));
        assert_eq!(it.next(), Some(b("bar")));
        assert_eq!(it.next_back(), Some(b("quux")));
        assert_eq!(it.next(), Some(b("baz")));
        assert_eq!(it.next_back(), None);
        assert_eq!(it.next(), None);
    }

    // Check that record equality respects field boundaries.
    //
    // Regression test for #138.
    #[test]
    fn eq_field_boundaries() {
        let test1 = ByteRecord::from(vec!["12", "34"]);
        let test2 = ByteRecord::from(vec!["123", "4"]);

        assert_ne!(test1, test2);
    }

    // Check that record equality respects number of fields.
    //
    // Regression test for #138.
    #[test]
    fn eq_record_len() {
        let test1 = ByteRecord::from(vec!["12", "34", "56"]);
        let test2 = ByteRecord::from(vec!["12", "34"]);
        assert_ne!(test1, test2);
    }
}
