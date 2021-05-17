#![allow(clippy::rc_buffer)]

use std::ptr::null_mut;
use std::slice;
use std::sync::atomic::{AtomicPtr, Ordering};
use std::sync::{Arc, Weak};

use crate::HashMap;

use crate::column::ColumnIndex;
use crate::error::Error;
use crate::ext::ustr::UStr;
use crate::row::Row;
use crate::sqlite::statement::StatementHandle;
use crate::sqlite::{Sqlite, SqliteColumn, SqliteValue, SqliteValueRef};

/// Implementation of [`Row`] for SQLite.
pub struct SqliteRow {
    // Raw handle of the SQLite statement
    // This is valid to access IFF the atomic [values] is null
    // The way this works is that the executor retains a weak reference to
    // [values] after the Row is created and yielded downstream.
    // IF the user drops the Row before iterating the stream (so
    // nearly all of our internal stream iterators), the executor moves on; otherwise,
    // it actually inflates this row with a list of owned sqlite3 values.
    pub(crate) statement: StatementHandle,

    pub(crate) values: Arc<AtomicPtr<SqliteValue>>,
    pub(crate) num_values: usize,

    pub(crate) columns: Arc<Vec<SqliteColumn>>,
    pub(crate) column_names: Arc<HashMap<UStr, usize>>,
}

impl crate::row::private_row::Sealed for SqliteRow {}

// Accessing values from the statement object is
// safe across threads as long as we don't call [sqlite3_step]

// we block ourselves from doing that by only exposing
// a set interface on [StatementHandle]

unsafe impl Send for SqliteRow {}
unsafe impl Sync for SqliteRow {}

impl SqliteRow {
    // creates a new row that is internally referencing the **current** state of the statement
    // returns a weak reference to an atomic list where the executor should inflate if its going
    // to increment the statement with [step]
    pub(crate) fn current(
        statement: StatementHandle,
        columns: &Arc<Vec<SqliteColumn>>,
        column_names: &Arc<HashMap<UStr, usize>>,
    ) -> (Self, Weak<AtomicPtr<SqliteValue>>) {
        let values = Arc::new(AtomicPtr::new(null_mut()));
        let weak_values = Arc::downgrade(&values);
        let size = statement.column_count();

        let row = Self {
            statement,
            values,
            num_values: size,
            columns: Arc::clone(columns),
            column_names: Arc::clone(column_names),
        };

        (row, weak_values)
    }

    // inflates this Row into memory as a list of owned, protected SQLite value objects
    // this is called by the
    #[allow(clippy::needless_range_loop)]
    pub(crate) fn inflate(
        statement: &StatementHandle,
        columns: &[SqliteColumn],
        values_ref: &AtomicPtr<SqliteValue>,
    ) {
        let size = statement.column_count();
        let mut values = Vec::with_capacity(size);

        for i in 0..size {
            values.push(unsafe {
                let raw = statement.column_value(i);

                SqliteValue::new(raw, columns[i].type_info.clone())
            });
        }

        // decay the array signifier and become just a normal, leaked array
        let values_ptr = Box::into_raw(values.into_boxed_slice()) as *mut SqliteValue;

        // store in the atomic ptr storage
        values_ref.store(values_ptr, Ordering::Release);
    }

    pub(crate) fn inflate_if_needed(
        statement: &StatementHandle,
        columns: &[SqliteColumn],
        weak_values_ref: Option<Weak<AtomicPtr<SqliteValue>>>,
    ) {
        if let Some(v) = weak_values_ref.and_then(|v| v.upgrade()) {
            SqliteRow::inflate(statement, &columns, &v);
        }
    }
}

impl Row for SqliteRow {
    type Database = Sqlite;

    fn columns(&self) -> &[SqliteColumn] {
        &self.columns
    }

    fn try_get_raw<I>(&self, index: I) -> Result<SqliteValueRef<'_>, Error>
    where
        I: ColumnIndex<Self>,
    {
        let index = index.index(self)?;

        let values_ptr = self.values.load(Ordering::Acquire);
        if !values_ptr.is_null() {
            // we have raw value data, we should use that
            let values: &[SqliteValue] =
                unsafe { slice::from_raw_parts(values_ptr, self.num_values) };

            Ok(SqliteValueRef::value(&values[index]))
        } else {
            Ok(SqliteValueRef::statement(
                &self.statement,
                self.columns[index].type_info.clone(),
                index,
            ))
        }
    }
}

impl Drop for SqliteRow {
    fn drop(&mut self) {
        // if there is a non-null pointer stored here, we need to re-load and drop it
        let values_ptr = self.values.load(Ordering::Acquire);
        if !values_ptr.is_null() {
            let values: &mut [SqliteValue] =
                unsafe { slice::from_raw_parts_mut(values_ptr, self.num_values) };

            let _ = unsafe { Box::from_raw(values) };
        }
    }
}

impl ColumnIndex<SqliteRow> for &'_ str {
    fn index(&self, row: &SqliteRow) -> Result<usize, Error> {
        row.column_names
            .get(*self)
            .ok_or_else(|| Error::ColumnNotFound((*self).into()))
            .map(|v| *v)
    }
}

#[cfg(feature = "any")]
impl From<SqliteRow> for crate::any::AnyRow {
    #[inline]
    fn from(row: SqliteRow) -> Self {
        crate::any::AnyRow {
            columns: row.columns.iter().map(|col| col.clone().into()).collect(),
            kind: crate::any::row::AnyRowKind::Sqlite(row),
        }
    }
}
